//! 录屏权限协议读/再查；假端口用测试环境，不真弹窗。

use service::{Call, RootHost};

#[tokio::test]
async fn screen_permission_reads_and_rechecks_with_the_fake_port() {
    let data = tempfile::tempdir().unwrap();
    let _cwd = tempfile::tempdir().unwrap();
    let host: RootHost = crate::common::open(crate::common::options(data.path())).await;
    service::set_test_screen_permission(Some("denied"));
    let first = host
        .dispatch(Call::ScreenPermission(service::ScreenPermissionRequest {}))
        .await
        .expect("read permission");
    let service::Response::ScreenPermission(first) = first else {
        panic!("not ScreenPermission");
    };
    assert_eq!(first.state, service::ScreenPermissionState::Denied);
    assert!(first.settings_url.contains("Privacy_ScreenCapture"));
    // 「再查一次」是同一个读接口；假端口仍回 denied。
    let again = host
        .dispatch(Call::ScreenPermission(service::ScreenPermissionRequest {}))
        .await
        .expect("recheck permission");
    let service::Response::ScreenPermission(again) = again else {
        panic!("not ScreenPermission");
    };
    assert_eq!(again.state, service::ScreenPermissionState::Denied);
    service::set_test_screen_permission(Some("granted"));
    let granted = host
        .dispatch(Call::ScreenPermission(service::ScreenPermissionRequest {}))
        .await
        .expect("granted permission");
    let service::Response::ScreenPermission(granted) = granted else {
        panic!("not ScreenPermission");
    };
    assert_eq!(granted.state, service::ScreenPermissionState::Granted);
    service::set_test_screen_permission(None);
    host.shutdown().await;
}

#[tokio::test]
async fn vision_test_reports_all_six_states_without_collapsing_failures() {
    use service::TestVisionOutcome;
    service::set_test_vision_outcomes(vec![
        TestVisionOutcome::Normal("红色".to_owned()),
        TestVisionOutcome::TextOnly,
        TestVisionOutcome::RateLimited(Some(3_000)),
        TestVisionOutcome::Timeout,
        TestVisionOutcome::Auth,
        TestVisionOutcome::Other("上游 500".to_owned()),
    ]);
    let data = tempfile::tempdir().unwrap();
    let host: RootHost = crate::common::open(crate::common::options(data.path())).await;
    let request = || service::VisionTestRequest {
        connection: "minimax-cn".to_owned(),
        model: "MiniMax-M3".to_owned(),
    };
    let call = |request: service::VisionTestRequest| Call::VisionTest(request);
    let check = |response: service::Response| {
        let service::Response::VisionTest(response) = response else {
            panic!("not VisionTest");
        };
        response
    };

    let normal = check(host.dispatch(call(request())).await.expect("normal"));
    assert_eq!(normal.state, "normal", "{normal:?}");
    assert!(normal.elapsed_ms.is_some());
    assert_eq!(normal.error_code, None);

    let text_only = check(host.dispatch(call(request())).await.expect("text-only"));
    assert_eq!(text_only.state, "text-only");
    assert_eq!(text_only.error_code, None);

    let rate = check(host.dispatch(call(request())).await.expect("rate"));
    assert_eq!(rate.state, "rate-limited");
    assert_eq!(rate.retry_after_ms, Some(3_000));

    let timeout = check(host.dispatch(call(request())).await.expect("timeout"));
    assert_eq!(timeout.state, "timeout");

    let auth = check(host.dispatch(call(request())).await.expect("auth"));
    assert_eq!(auth.state, "error");
    assert_eq!(auth.error_code.as_deref(), Some("VISION_AUTH_FAILED"));
    assert_ne!(auth.state, "text-only");

    let other = check(host.dispatch(call(request())).await.expect("other"));
    assert_eq!(other.state, "error");
    assert_eq!(other.error_code.as_deref(), Some("VISION_BACKEND_ERROR"));
    assert_eq!(other.message.as_deref(), Some("上游 500"));
    assert_ne!(other.state, "text-only");

    service::set_test_vision_outcomes(Vec::new());
    host.shutdown().await;
}
