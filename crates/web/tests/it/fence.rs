//! T006.17 红测试：信任栅栏（接缝 4）+ ready/事件帧顺序与重连补齐。
//!
//! DSH 对照：connection README 第 41/47 行（每个进程一个启动令牌；Host 必须 loopback 或绑定
//! IP；Host 失败 403、令牌失败 401；0.0.0.0 不支持）、webserver README 第 39 行（未指定地址
//! 拒绝）、Connection generation（ready 项先于事件、重连带最后序号重新跟随）。

use axum::http::{HeaderMap, header::HOST};
use service::{ContextUsage, FollowResponse, HostInfo, RootState, ServiceEvent};
use web::{
    Frame, Trust, authorize_connect, check_api_auth, check_bind_host, opening_frames,
    reject_query_token,
};

use crate::common::{headers, trust};

fn event(seq: u64, event_type: &str) -> ServiceEvent {
    ServiceEvent {
        seq,
        time: 1,
        event_type: event_type.to_owned(),
        data: serde_json::json!({"seq": seq}),
        cause: None,
    }
}

fn info() -> HostInfo {
    HostInfo {
        protocol_version: service::PROTOCOL_VERSION,
        session_id: Some("sess-1".to_owned()),
        root: RootState::Ok,
        context: ContextUsage {
            used: 0,
            window: 1000,
        },
        host: service::HostLocation {
            home: std::env::var("HOME").ok().filter(|home| !home.is_empty()),
        },
    }
}

#[test]
fn bind_fence_allows_only_loopback() {
    assert_eq!(
        check_bind_host("127.0.0.1").unwrap(),
        std::net::IpAddr::from([127, 0, 0, 1])
    );
    assert_eq!(
        check_bind_host("::1").unwrap(),
        std::net::IpAddr::from(std::net::Ipv6Addr::LOCALHOST)
    );
    // 0.0.0.0 / 非回环 / 非 IP 字面量都拒绝启动（DSH webserver README 第 39 行）。
    for host in ["0.0.0.0", "::", "8.8.8.8", "localhost", "example.com"] {
        assert!(
            matches!(
                check_bind_host(host),
                Err(web::TrustError::UnsupportedBind(_))
            ),
            "{host} 必须被拒绝"
        );
    }
}

fn auth_headers(host: Option<&str>, cookie: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(host) = host {
        headers.insert(HOST, host.parse().expect("Host"));
    }
    if let Some(cookie) = cookie {
        headers.insert("cookie", cookie.parse().expect("cookie"));
    }
    headers
}

/// 用启动令牌换一次签名 cookie（fence 测试统一走 cookie 入口，照 T006.F2）。
fn cookie_for(trust: &Trust, host: &str) -> String {
    let grant = authorize_connect(trust, Some(host), Some("tok-1")).expect("正确令牌换 cookie");
    grant
        .set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie 不能为空")
        .to_owned()
}

#[test]
fn host_fence_rejects_external_host_header() {
    let trust = trust();
    // loopback 的 Host 头（带任意端口）放行（cookie 绑同一个 authority）。
    for host in [
        "127.0.0.1",
        "127.0.0.1:1417",
        "localhost:9999",
        "[::1]:1417",
    ] {
        let cookie = cookie_for(&trust, host);
        assert!(
            check_api_auth(&trust, &auth_headers(Some(host), Some(&cookie))).is_ok(),
            "Host {host} 应放行"
        );
    }
    // 外部 Host 头 403。
    for host in [
        "evil.example:80",
        "8.8.8.8:1417",
        "0.0.0.0:1417",
        "127.0.0.1.evil:1417",
    ] {
        let cookie = cookie_for(&trust, "127.0.0.1:1417");
        let error = check_api_auth(&trust, &auth_headers(Some(host), Some(&cookie))).unwrap_err();
        assert_eq!(error.status(), 403, "Host {host} 必须 403");
        assert!(
            matches!(error, web::TrustError::BadHost(_)),
            "{host} 应是 BadHost"
        );
    }
    let cookie = cookie_for(&trust, "127.0.0.1:1417");
    assert!(matches!(
        check_api_auth(&trust, &auth_headers(None, Some(&cookie))),
        Err(web::TrustError::BadHost(_))
    ));
}

#[test]
fn cookie_fence_requires_valid_cookie() {
    let trust = trust();
    let authority = "127.0.0.1:1417";
    let error = check_api_auth(&trust, &auth_headers(Some(authority), None)).unwrap_err();
    assert_eq!(error.status(), 401, "没有 cookie 必须 401");
    assert!(matches!(error, web::TrustError::Unauthorized));
    let error =
        check_api_auth(&trust, &auth_headers(Some(authority), Some("other=1"))).unwrap_err();
    assert_eq!(error.status(), 401, "cookie 名字不对必须 401");
    let cookie = cookie_for(&trust, authority);
    let (name, value) = cookie.split_once('=').expect("cookie 有名字");
    let error = check_api_auth(
        &trust,
        &auth_headers(Some(authority), Some(&format!("{name}={value}bad"))),
    )
    .unwrap_err();
    assert_eq!(error.status(), 401, "坏签名 cookie 必须 401");
}

/// 公开面回归：trust 拆成私有模块后，crate 外仍要能调用 `web::reject_query_token`
/// （这条用例编译不过，就说明重导出又丢了）。
#[test]
fn query_token_guard_is_reachable_from_outside_the_crate() {
    let plain: axum::http::Uri = "http://127.0.0.1:1417/api".parse().expect("Uri");
    assert!(reject_query_token(&plain).is_ok());
    let tokened: axum::http::Uri = "http://127.0.0.1:1417/api?token=tok-1"
        .parse()
        .expect("Uri");
    assert!(matches!(
        reject_query_token(&tokened),
        Err(web::TrustError::Unauthorized)
    ));
}

#[test]
fn opening_sends_ready_before_events_in_order() {
    let follow = FollowResponse {
        events: vec![event(3, "turn/start"), event(4, "user/message")],
        last_seq: 4,
    };
    let frames = opening_frames(&info(), follow);
    assert_eq!(frames.len(), 3, "ready + 两条事件");
    assert!(
        matches!(&frames[0], Frame::Ready { session_id: Some(id), .. } if id == "sess-1"),
        "第一帧必须是 ready：{frames:?}"
    );
    let seqs: Vec<u64> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Event { event } => Some(event.seq),
            Frame::Ready { .. } | Frame::Agent { .. } => None,
        })
        .collect();
    assert_eq!(seqs, vec![3, 4], "事件按 seq 顺序、ready 之后");

    // 线上形状照 DSH：ready {type:'ready',...}；事件 {type:'event', event:{seq,type,data}}。
    let mut expected_ready = serde_json::json!({
        "type": "ready",
        "sessionId": "sess-1",
        "protocolVersion": service::PROTOCOL_VERSION,
        "root": {"state": "ok"},
        "context": {"used": 0, "window": 1000},
    });
    // T009.5：ready 带 host: {home}（测试进程的 HOME；没有就省略字段）。
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => expected_ready["host"] = serde_json::json!({"home": home}),
        _ => expected_ready["host"] = serde_json::json!({}),
    }
    assert_eq!(serde_json::to_value(&frames[0]).unwrap(), expected_ready);
    assert_eq!(
        serde_json::to_value(&frames[1]).unwrap(),
        serde_json::json!({
            "type": "event",
            "event": {"seq": 3, "time": 1, "type": "turn/start", "data": {"seq": 3}}
        })
    );
}

#[test]
fn reconnect_follows_from_last_seq_without_duplicates() {
    // 断线重连：带 lastSeq=4 重新打开，只应拿到 ready（没有新事件），不能重复 3/4。
    let follow = FollowResponse {
        events: Vec::new(),
        last_seq: 4,
    };
    let frames = opening_frames(&info(), follow);
    assert_eq!(frames.len(), 1, "没有新事件时只有 ready：{frames:?}");
    assert!(matches!(frames[0], Frame::Ready { .. }));

    // 有后续事件时，从 lastSeq 之后开始（由 service follow 保证 seq > lastSeq，这里验证帧顺序）。
    let follow = FollowResponse {
        events: vec![event(5, "step/start"), event(6, "assistant/message")],
        last_seq: 6,
    };
    let frames = opening_frames(&info(), follow);
    let seqs: Vec<u64> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Event { event } => Some(event.seq),
            Frame::Ready { .. } | Frame::Agent { .. } => None,
        })
        .collect();
    assert_eq!(seqs, vec![5, 6], "重连补齐从 lastSeq 之后开始、不重复");
}

// T006.F2 红测试：Origin / sec-fetch-site 栅栏（先骨架，随后实现）。
#[test]
fn cross_site_origin_is_rejected() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("origin", "https://evil.example".parse().unwrap());
    let error = web::check_origin(&headers).unwrap_err();
    assert_eq!(error.status(), 403);
    assert!(matches!(error, web::TrustError::BadOrigin(_)));
}

#[test]
fn sec_fetch_cross_site_is_rejected_without_origin() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
    let error = web::check_origin(&headers).unwrap_err();
    assert_eq!(error.status(), 403);
    assert!(matches!(error, web::TrustError::BadOrigin(_)));
}

#[test]
fn same_origin_is_allowed() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("origin", "http://127.0.0.1:1417".parse().unwrap());
    headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
    assert!(web::check_origin(&headers).is_ok());
}

#[test]
fn null_or_malformed_origin_is_rejected() {
    for origin in ["null", "http://", "not a url"] {
        let mut headers = headers("127.0.0.1:1417");
        headers.insert("origin", origin.parse().unwrap());
        let error = web::check_origin(&headers).unwrap_err();
        assert_eq!(error.status(), 403, "Origin {origin} 必须 403");
    }
}

#[test]
fn cross_port_origin_is_rejected() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("origin", "http://127.0.0.1:1418".parse().unwrap());
    assert!(matches!(
        web::check_origin(&headers),
        Err(web::TrustError::BadOrigin(_))
    ));
}

#[test]
fn missing_origin_and_fetch_metadata_is_allowed() {
    let headers = headers("127.0.0.1:1417");
    assert!(web::check_origin(&headers).is_ok());
}
