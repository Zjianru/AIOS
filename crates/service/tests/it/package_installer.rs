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

#[tokio::test]
async fn package_status_maps_downloading_verifying_installed_and_failed() {
    use service::installer::{Installer, find};
    let spec = find("pp-ocrv6-small").expect("manifest");

    let case = |state: serde_json::Value| {
        let data = tempfile::tempdir().unwrap();
        let installer = Installer::new(data.path().to_path_buf());
        let root = installer.version_root(spec);
        match state["kind"].as_str().unwrap_or_default() {
            "installed" => {
                std::fs::create_dir_all(&root).unwrap();
                for file in &spec.files {
                    std::fs::write(root.join(&file.target), b"x").unwrap();
                }
            }
            _ => {
                let tmp = root.join("tmp");
                std::fs::create_dir_all(&tmp).unwrap();
                std::fs::write(tmp.join("status.json"), state.to_string()).unwrap();
            }
        }
        (data, installer)
    };

    let (downloading_dir, _) = case(serde_json::json!({
        "state": "downloading", "done": 10, "total": 100
    }));
    let (verifying_dir, _) = case(serde_json::json!({
        "state": "verifying", "done": 100, "total": 100
    }));
    let (installed_dir, _) = case(serde_json::json!({"kind": "installed"}));
    let (failed_dir, _) = case(serde_json::json!({
        "state": "failed", "reason": "sha256 不符"
    }));

    async fn status_of(data: &std::path::Path) -> service::PackageStatusView {
        let host: RootHost = crate::common::open(crate::common::options(data)).await;
        let response = host
            .dispatch(Call::PackageStatus(service::PackageStatusRequest {}))
            .await
            .expect("status");
        let service::Response::PackageStatus(response) = response else {
            panic!("not PackageStatus");
        };
        host.shutdown().await;
        response
            .packages
            .into_iter()
            .find(|package| package.id == "pp-ocrv6-small")
            .expect("ocr package")
    }

    let downloading = status_of(downloading_dir.path()).await;
    assert_eq!(downloading.state, "downloading");
    assert_eq!(downloading.done_bytes, 10);
    assert_eq!(downloading.total_bytes, 100);

    let verifying = status_of(verifying_dir.path()).await;
    assert_eq!(verifying.state, "verifying");

    let installed = status_of(installed_dir.path()).await;
    assert_eq!(installed.state, "installed");
    assert!(installed.installed_at.is_some());

    let failed = status_of(failed_dir.path()).await;
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.reason.as_deref(), Some("sha256 不符"));
}
