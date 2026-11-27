//! 通用安装卡的确认动作：只认内置清单，点了才下载。

use std::path::PathBuf;

use serde_json::json;

use super::{ActionEnv, ActionFuture, ActionKind, ActionOutcome, CardFact};
use crate::installer::{self, Installer};
use crate::root_lifecycle::RootHost;

pub(crate) struct InstallPackageAction {
    data_dir: PathBuf,
    /// 测试时注入本地 mock 清单；生产 None，只认内置清单。
    spec_override: Option<installer::PackageSpec>,
}

impl InstallPackageAction {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            spec_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_spec(data_dir: PathBuf, spec: installer::PackageSpec) -> Self {
        Self {
            data_dir,
            spec_override: Some(spec),
        }
    }

    fn spec_for(&self, id: &str) -> Option<installer::PackageSpec> {
        self.spec_override
            .clone()
            .or_else(|| installer::find(id).cloned())
    }
}

fn human_size(bytes: u64) -> String {
    let mib = bytes as f64 / 1_048_576.0;
    format!("{mib:.1} MiB（{bytes} 字节）")
}

impl ActionKind for InstallPackageAction {
    fn name(&self) -> &str {
        "install_package"
    }

    fn skill(&self) -> &str {
        "aios-settings"
    }

    fn details(&self, raw: &serde_json::Value) -> Result<serde_json::Value, String> {
        let id = raw
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "缺少包 id".to_owned())?;
        if self.spec_for(id).is_none() {
            return Err(format!("清单外包：{id}"));
        }
        Ok(json!({ "id": id }))
    }

    fn describe(
        &self,
        details: &serde_json::Value,
        _env: &ActionEnv<'_>,
    ) -> Result<Vec<CardFact>, String> {
        let id = details
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "缺少包 id".to_owned())?;
        let spec = self.spec_for(id).ok_or_else(|| format!("清单外包：{id}"))?;
        let status = Installer::new(self.data_dir.clone()).status(id);
        if matches!(status.state, installer::PackageState::Installed { .. }) {
            return Err("这个包已经装好了".to_owned());
        }
        Ok(vec![
            CardFact {
                name: "包".to_owned(),
                value: spec.name.clone(),
            },
            CardFact {
                name: "版本".to_owned(),
                value: spec.version.clone(),
            },
            CardFact {
                name: "大小".to_owned(),
                value: human_size(spec.total_bytes),
            },
            CardFact {
                name: "来源".to_owned(),
                value: spec.source.clone(),
            },
            CardFact {
                name: "许可".to_owned(),
                value: spec.license.clone(),
            },
            CardFact {
                name: "装到哪".to_owned(),
                value: Installer::new(self.data_dir.clone())
                    .version_root(&spec)
                    .display()
                    .to_string(),
            },
            CardFact {
                name: "用途".to_owned(),
                value: spec.purpose.clone(),
            },
            CardFact {
                name: "核对".to_owned(),
                value: "逐个文件核对 sha256，全部通过才落盘；失败不覆盖旧版本。".to_owned(),
            },
        ])
    }

    fn act<'a>(
        &'a self,
        details: &'a serde_json::Value,
        _host: &'a RootHost,
    ) -> ActionFuture<'a, Result<ActionOutcome, String>> {
        Box::pin(async move { self.run(details).await })
    }
}

impl InstallPackageAction {
    pub(crate) async fn run(&self, details: &serde_json::Value) -> Result<ActionOutcome, String> {
        let id = details
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "缺少包 id".to_owned())?;
        let spec = self.spec_for(id).ok_or_else(|| format!("清单外包：{id}"))?;
        let installer = Installer::new(self.data_dir.clone());
        installer.install_spec(&spec).await?;
        let status = installer.status(id);
        Ok(ActionOutcome {
            summary: format!("{} 已装好（{}）。", spec.name, spec.version),
            data: json!({
                "package": installer::status_view(&status),
                "note": "下载并核对完成",
                "progress": 1.0,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &std::path::Path) -> ActionEnv<'_> {
        ActionEnv {
            data_dir: dir,
            identity: None,
            events: &[],
            conversation: None,
        }
    }

    #[test]
    fn unknown_ids_are_rejected_before_a_card_is_built() {
        let dir = tempfile::tempdir().unwrap();
        let action = InstallPackageAction::new(dir.path().to_path_buf());
        let error = action
            .details(&json!({"id": "evil"}))
            .expect_err("清单外 id");
        assert!(error.contains("清单外"), "{error}");
        assert!(action.details(&json!({"id": "pp-ocrv6-small"})).is_ok());
    }

    #[test]
    fn describe_shows_name_version_size_source_location_and_license() {
        let dir = tempfile::tempdir().unwrap();
        let action = InstallPackageAction::new(dir.path().to_path_buf());
        let details = action
            .details(&json!({"id": "pp-ocrv6-small"}))
            .expect("id");
        let facts = action.describe(&details, &env(dir.path())).expect("facts");
        let names: Vec<&str> = facts.iter().map(|fact| fact.name.as_str()).collect();
        for required in [
            "包",
            "版本",
            "大小",
            "来源",
            "许可",
            "装到哪",
            "用途",
            "核对",
        ] {
            assert!(names.contains(&required), "缺 {required}: {names:?}");
        }
        let text = facts
            .iter()
            .map(|fact| format!("{}={}", fact.name, fact.value))
            .collect::<Vec<_>>()
            .join(";");
        assert!(text.contains("Apache-2.0"), "{text}");
        assert!(text.contains("MiB"), "{text}");
    }

    #[tokio::test]
    async fn act_returns_note_and_progress_after_install() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let body = b"install-package-test";
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
        let sha = {
            let mut hex = String::new();
            for byte in ring::digest::digest(&ring::digest::SHA256, body).as_ref() {
                hex.push_str(&format!("{byte:02x}"));
            }
            hex
        };
        let spec = crate::installer::PackageSpec {
            id: "pp-ocrv6-small".to_owned(),
            name: "测试 OCR 包".to_owned(),
            version: "v-test".to_owned(),
            purpose: "测试".to_owned(),
            source: "mock".to_owned(),
            license: "Apache-2.0".to_owned(),
            total_bytes: body.len() as u64,
            files: vec![crate::installer::PackageFile {
                url: format!("{}/file.bin", server.uri()),
                sha256: sha,
                size: body.len() as u64,
                target: "file.bin".to_owned(),
                version: "v-test".to_owned(),
            }],
        };
        let dir = tempfile::tempdir().unwrap();
        let action = InstallPackageAction::new_with_spec(dir.path().to_path_buf(), spec);
        let outcome = action
            .run(&json!({"id": "pp-ocrv6-small"}))
            .await
            .expect("act");
        assert_eq!(outcome.data["note"], "下载并核对完成");
        assert_eq!(outcome.data["progress"], 1.0);
        assert!(outcome.summary.contains("已装好"));
        assert!(outcome.data["package"].is_object());
    }
}
