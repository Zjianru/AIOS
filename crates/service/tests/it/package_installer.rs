//! 包状态/卸载走协议；下载/sha 的 mock 在 installer 单测里。

use service::{Call, RootHost};

#[tokio::test]
async fn package_status_lists_the_pinned_ocr_package_and_uninstall_rejects_unknown() {
    let data = tempfile::tempdir().unwrap();
    let host: RootHost = crate::common::open(crate::common::options(data.path())).await;
    let response = host
        .dispatch(Call::PackageStatus(service::PackageStatusRequest {}))
        .await
        .expect("包状态");
    let service::Response::PackageStatus(response) = response else {
        panic!("not PackageStatus");
    };
    let ocr = response
        .packages
        .iter()
        .find(|package| package.id == "pp-ocrv6-small")
        .expect("OCR 包在清单里");
    assert_eq!(ocr.state, "not-installed");
    assert!(ocr.version.contains("28fe5895"));
    assert_eq!(ocr.total_bytes, 0);
    let error = host
        .dispatch(Call::PackageUninstall(service::PackageUninstallRequest {
            id: "evil".to_owned(),
        }))
        .await
        .expect_err("清单外包不能卸载");
    assert!(error.to_string().contains("清单外包"), "{error}");
    host.shutdown().await;
}
