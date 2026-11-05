//! T012.1 测试清单 12：代理——设了 http_proxy 指向 mock，请求本机地址，mock 收到 0 次（故事 9）。

use model::http::{HttpEnv, client_with_env};
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
