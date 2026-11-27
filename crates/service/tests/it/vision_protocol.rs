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
