//! T006.17 红测试：信任栅栏（接缝 4）+ ready/事件帧顺序与重连补齐。
//!
//! DSH 对照：connection README 第 41/47 行（每个进程一个启动令牌；Host 必须 loopback 或绑定
//! IP；Host 失败 403、令牌失败 401；0.0.0.0 不支持）、webserver README 第 39 行（未指定地址
//! 拒绝）、Connection generation（ready 项先于事件、重连带最后序号重新跟随）。

use service::{FollowResponse, ServiceEvent};
use web::{Frame, Trust, check_bind_host, check_request, opening_frames};

fn trust() -> Trust {
    Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
    )
}

fn event(seq: u64, event_type: &str) -> ServiceEvent {
    ServiceEvent {
        seq,
        event_type: event_type.to_owned(),
        data: serde_json::json!({"seq": seq}),
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

#[test]
fn host_fence_rejects_external_host_header() {
    let trust = trust();
    // loopback 的 Host 头（带任意端口）放行。
    for host in [
        "127.0.0.1",
        "127.0.0.1:1417",
        "localhost:9999",
        "[::1]:1417",
    ] {
        assert!(
            check_request(&trust, Some(host), Some("tok-1")).is_ok(),
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
        let error = check_request(&trust, Some(host), Some("tok-1")).unwrap_err();
        assert_eq!(error.status(), 403, "Host {host} 必须 403");
        assert!(
            matches!(error, web::TrustError::BadHost(_)),
            "{host} 应是 BadHost"
        );
    }
    assert!(matches!(
        check_request(&trust, None, Some("tok-1")),
        Err(web::TrustError::BadHost(_))
    ));
}

#[test]
fn token_fence_requires_launch_token() {
    let trust = trust();
    let error = check_request(&trust, Some("127.0.0.1:1417"), None).unwrap_err();
    assert_eq!(error.status(), 401, "没有令牌必须 401");
    assert!(matches!(error, web::TrustError::Unauthorized));
    let error = check_request(&trust, Some("127.0.0.1:1417"), Some("tok-2")).unwrap_err();
    assert_eq!(error.status(), 401, "错误令牌必须 401");
    let error = check_request(&trust, Some("127.0.0.1:1417"), Some("tok-10000")).unwrap_err();
    assert_eq!(error.status(), 401, "长度不同的令牌也必须 401");
}

#[test]
fn opening_sends_ready_before_events_in_order() {
    let follow = FollowResponse {
        events: vec![event(3, "turn/start"), event(4, "user/message")],
        last_seq: 4,
    };
    let frames = opening_frames("sess-1", follow);
    assert_eq!(frames.len(), 3, "ready + 两条事件");
    assert!(
        matches!(&frames[0], Frame::Ready { session_id } if session_id == "sess-1"),
        "第一帧必须是 ready：{frames:?}"
    );
    let seqs: Vec<u64> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Event { event } => Some(event.seq),
            Frame::Ready { .. } => None,
        })
        .collect();
    assert_eq!(seqs, vec![3, 4], "事件按 seq 顺序、ready 之后");

    // 线上形状照 DSH：ready {type:'ready',...}；事件 {type:'event', event:{seq,type,data}}。
    assert_eq!(
        serde_json::to_value(&frames[0]).unwrap(),
        serde_json::json!({"type": "ready", "sessionId": "sess-1"})
    );
    assert_eq!(
        serde_json::to_value(&frames[1]).unwrap(),
        serde_json::json!({
            "type": "event",
            "event": {"seq": 3, "type": "turn/start", "data": {"seq": 3}}
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
    let frames = opening_frames("sess-1", follow);
    assert_eq!(frames.len(), 1, "没有新事件时只有 ready：{frames:?}");
    assert!(matches!(frames[0], Frame::Ready { .. }));

    // 有后续事件时，从 lastSeq 之后开始（由 service follow 保证 seq > lastSeq，这里验证帧顺序）。
    let follow = FollowResponse {
        events: vec![event(5, "step/start"), event(6, "assistant/message")],
        last_seq: 6,
    };
    let frames = opening_frames("sess-1", follow);
    let seqs: Vec<u64> = frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::Event { event } => Some(event.seq),
            Frame::Ready { .. } => None,
        })
        .collect();
    assert_eq!(seqs, vec![5, 6], "重连补齐从 lastSeq 之后开始、不重复");
}
