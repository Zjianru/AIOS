//! T012.1 测试清单 12：代理——设了 http_proxy 指向 mock，请求本机地址，mock 收到 0 次（故事 9）。

use model::http::{HttpEnv, client_with_env};
use reqwest::Url;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn local_address_bypasses_proxy_but_other_addresses_use_it() {
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&target)
        .await;
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&proxy)
        .await;
    let proxy_uri = proxy.uri();
    let env = HttpEnv::from_source(move |key| match key {
        "http_proxy" | "HTTP_PROXY" => Some(proxy_uri.clone()),
        _ => None,
    });
    let client = client_with_env(env).expect("造 client");

    // 本机地址：必须直连，代理收到 0 次。
    let local = client.get(target.uri()).send().await.expect("本机请求");
    assert_eq!(local.status(), 200);
    assert_eq!(
        target
            .received_requests()
            .await
            .expect("目标请求记录")
            .len(),
        1,
        "本机目标应被直接访问"
    );
    assert_eq!(
        proxy.received_requests().await.expect("代理请求记录").len(),
        0,
        "本机地址不能走代理（故事 9）"
    );

    // 非本机地址：照环境变量走代理。
    let proxied = client.get("http://example.invalid/ping").send().await;
    assert!(proxied.is_ok(), "代理应返回 200：{proxied:?}");
    assert!(
        !proxy
            .received_requests()
            .await
            .expect("代理请求记录")
            .is_empty(),
        "非本机地址应走代理"
    );
}

/// T012.1 审计 fix-now：IPv6 ::1 的 host_str 带方括号，必须仍按本机地址直连；
/// localhost 与 127.0.0.0/8 一并覆盖（故事 9）。
#[tokio::test]
async fn ipv6_and_named_loopback_addresses_bypass_proxy() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&proxy)
        .await;
    let proxy_uri = proxy.uri();
    let env = HttpEnv::from_source(move |key| match key {
        "http_proxy" | "HTTP_PROXY" => Some(proxy_uri.clone()),
        _ => None,
    });
    let client = client_with_env(env).expect("造 client");
    for url in [
        "http://localhost:9/",
        "http://127.1.2.3:9/",
        "http://[::1]:9/",
        "http://[::ffff:127.0.0.1]:9/",
    ] {
        let before = proxy.received_requests().await.expect("代理记录").len();
        #[expect(
            clippy::let_underscore_must_use,
            reason = "坏代理请求预期失败，只核代理是否收到请求"
        )]
        let _ = client
            .get(url)
            .timeout(std::time::Duration::from_millis(500))
            .send()
            .await;
        let after = proxy.received_requests().await.expect("代理记录").len();
        assert_eq!(after, before, "本机地址不能走代理：{url}");
    }
}
/// 代理地址解析失败不能再静默直连：造 client 时就把坏地址报出来，并指名是哪个变量。
#[test]
fn proxy_resolve_fail() {
    let env = HttpEnv::from_source(|key| match key {
        "https_proxy" | "HTTPS_PROXY" => Some("http://[::1".to_owned()),
        _ => None,
    });
    let error = client_with_env(env).expect_err("坏代理地址必须报错，不能静默直连");
    assert_eq!(error.code, model::ErrorCode::Transport);
    assert!(
        error.message.contains("https_proxy"),
        "报错要指名是哪个代理变量：{}",
        error.message
    );
}

fn proxy_host_port(url: &Url) -> (&str, u16) {
    (
        url.host_str().expect("代理主机"),
        url.port_or_known_default().expect("代理端口"),
    )
}

/// 本机地址（localhost、127/8、::1、v4-mapped）不走代理。
#[test]
fn proxy_for_bypasses_loopback_addresses() {
    let env = HttpEnv {
        http_proxy: Some("http://proxy.example:8080".to_owned()),
        ..HttpEnv::default()
    };
    for target in [
        "http://localhost:9/",
        "http://127.1.2.3:9/",
        "http://[::1]:9/",
        "http://[::ffff:127.0.0.1]:9/",
    ] {
        assert!(
            env.proxy_for(&Url::parse(target).expect("URL"))
                .expect("选代理")
                .is_none(),
            "本机地址不走代理：{target}"
        );
    }
}

/// no_proxy 命中的主机（含子域名和 IP）直连。
#[test]
fn proxy_for_honours_no_proxy() {
    let env = HttpEnv {
        http_proxy: Some("http://proxy.example:8080".to_owned()),
        no_proxy: Some("example.test, .corp.test, 127.0.0.1".to_owned()),
        ..HttpEnv::default()
    };
    for target in [
        "http://example.test/",
        "http://api.example.test/",
        "http://host.corp.test/",
        "http://127.0.0.1:9/",
    ] {
        assert!(
            env.proxy_for(&Url::parse(target).expect("URL"))
                .expect("选代理")
                .is_none(),
            "no_proxy 命中要直连：{target}"
        );
    }
    let proxied = env
        .proxy_for(&Url::parse("http://other.test/").expect("URL"))
        .expect("选代理")
        .expect("没命中 no_proxy 要走代理");
    assert_eq!(proxy_host_port(&proxied), ("proxy.example", 8080));
}

/// 按目标协议挑代理：http 用 http_proxy，https 用 https_proxy。
#[test]
fn proxy_for_selects_the_proxy_for_the_scheme() {
    let env = HttpEnv {
        http_proxy: Some("http://http-proxy.example:8001".to_owned()),
        https_proxy: Some("http://https-proxy.example:8002".to_owned()),
        all_proxy: Some("http://all-proxy.example:8003".to_owned()),
        no_proxy: None,
    };
    let http = env
        .proxy_for(&Url::parse("http://target.test/").expect("URL"))
        .expect("选代理")
        .expect("http 代理");
    assert_eq!(proxy_host_port(&http), ("http-proxy.example", 8001));
    let https = env
        .proxy_for(&Url::parse("https://target.test/").expect("URL"))
        .expect("选代理")
        .expect("https 代理");
    assert_eq!(proxy_host_port(&https), ("https-proxy.example", 8002));
}

/// 没有对应的协议代理时用 all_proxy 兜底。
#[test]
fn proxy_for_falls_back_to_all_proxy() {
    let env = HttpEnv {
        http_proxy: None,
        https_proxy: None,
        all_proxy: Some("http://all-proxy.example:8003".to_owned()),
        no_proxy: None,
    };
    for target in ["http://target.test/", "https://target.test/"] {
        let proxy = env
            .proxy_for(&Url::parse(target).expect("URL"))
            .expect("选代理")
            .expect("all_proxy 兜底");
        assert_eq!(proxy_host_port(&proxy), ("all-proxy.example", 8003));
    }
}

/// 坏代理地址要报错（指名变量），不能当成没配代理而静默直连。
#[test]
fn proxy_for_reports_a_bad_proxy_address() {
    let env = HttpEnv {
        http_proxy: Some("http://[::1".to_owned()),
        ..HttpEnv::default()
    };
    let error = env
        .proxy_for(&Url::parse("http://target.test/").expect("URL"))
        .expect_err("坏代理地址要报错，不能静默直连");
    assert!(
        error.message.contains("http_proxy"),
        "报错要指名哪个变量：{}",
        error.message
    );
}
