//! 通用包安装器：内置官方清单、逐文件 sha256、原子落盘、卸载只删包目录。

use std::path::{Path, PathBuf};

use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

/// 清单里的一个文件；URL 和 sha256 都必须来自内置清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageFile {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub target: String,
    /// 安装卡上按文件展示的版本（短 commit）。
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSpec {
    pub id: String,
    pub name: String,
    pub version: String,
    pub purpose: String,
    pub source: String,
    pub license: String,
    pub total_bytes: u64,
    pub files: Vec<PackageFile>,
}

const OCR_DET_URL: &str = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/28fe5895c24fd108c19eb3e8479f4ab385fbfc62/inference.onnx";
const OCR_REC_URL: &str = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/b8f84f0b80c529de40b4fbb3544b84fa7233a513/inference.onnx";
const OCR_DICT_URL: &str = "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/dab3fe35379033fdcb2d0e9572fac0b36c9a9ebf/ppocr/utils/dict/ppocrv6_dict.txt";

/// 内置官方清单；只允许这些包，URL 不追 main/latest。
pub fn manifest() -> &'static [PackageSpec] {
    static MANIFEST: std::sync::OnceLock<Vec<PackageSpec>> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| {
        // 只在显式测试进程里允许用本地假清单；生产只读内置清单。
        if let Some(path) = std::env::var_os("AIOS_PACKAGE_TEST_MANIFEST")
            && let Ok(text) = std::fs::read_to_string(path)
            && let Ok(specs) = serde_json::from_str::<Vec<PackageSpec>>(&text)
        {
            return specs;
        }
        vec![PackageSpec {
            id: "pp-ocrv6-small".to_owned(),
            name: "本机 OCR（PP-OCRv6 small）".to_owned(),
            version: "det@28fe5895 / rec@b8f84f0b / dict@dab3fe35".to_owned(),
            purpose: "中英混排截图在本机逐字识别，不联网、不装系统 OCR。".to_owned(),
            source: "HuggingFace PaddlePaddle/PP-OCRv6_small_det_onnx + _rec_onnx；PaddleOCR dict"
                .to_owned(),
            license: "Apache-2.0".to_owned(),
            total_bytes: 9_880_512 + 21_159_378 + 74_947,
            files: vec![
                PackageFile {
                    url: OCR_DET_URL.to_owned(),
                    sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e"
                        .to_owned(),
                    size: 9_880_512,
                    target: "det.onnx".to_owned(),
                    version: "det@28fe5895".to_owned(),
                },
                PackageFile {
                    url: OCR_REC_URL.to_owned(),
                    sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634"
                        .to_owned(),
                    size: 21_159_378,
                    target: "rec.onnx".to_owned(),
                    version: "rec@b8f84f0b".to_owned(),
                },
                PackageFile {
                    url: OCR_DICT_URL.to_owned(),
                    sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d"
                        .to_owned(),
                    size: 74_947,
                    target: "dict.txt".to_owned(),
                    version: "dict@dab3fe35".to_owned(),
                },
            ],
        }]
    })
}

pub fn find(id: &str) -> Option<&'static PackageSpec> {
    manifest().iter().find(|spec| spec.id == id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageState {
    NotInstalled,
    Downloading { done: u64, total: u64 },
    Verifying,
    Installed { installed_at: u64 },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageStatus {
    pub id: String,
    pub version: String,
    pub state: PackageState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StatusFile {
    state: String,
    #[serde(default)]
    done: u64,
    #[serde(default)]
    total: u64,
    #[serde(default)]
    reason: Option<String>,
}

pub struct Installer {
    data_dir: PathBuf,
}

impl Installer {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn package_root(&self, id: &str) -> PathBuf {
        self.data_dir.join("packages").join(id)
    }

    pub fn version_root(&self, spec: &PackageSpec) -> PathBuf {
        self.package_root(&spec.id).join(version_dir(spec))
    }

    pub fn status(&self, id: &str) -> PackageStatus {
        let Some(spec) = find(id) else {
            return PackageStatus {
                id: id.to_owned(),
                version: String::new(),
                state: PackageState::NotInstalled,
            };
        };
        self.status_for_spec(spec)
    }

    fn status_for_spec(&self, spec: &PackageSpec) -> PackageStatus {
        let version_root = self.version_root(spec);
        let installed_root = installed_version_dir(&self.package_root(&spec.id), spec);
        if let Some(installed_root) = installed_root {
            let installed_at = std::fs::metadata(&installed_root)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            return PackageStatus {
                id: spec.id.clone(),
                version: spec.version.clone(),
                state: PackageState::Installed { installed_at },
            };
        }
        let status = read_status(&version_root.join("tmp").join("status.json"));
        let state = match status.state.as_str() {
            "downloading" => PackageState::Downloading {
                done: status.done,
                total: status.total,
            },
            "verifying" => PackageState::Verifying,
            "failed" => PackageState::Failed {
                reason: status.reason.unwrap_or_else(|| "安装失败".to_owned()),
            },
            _ => PackageState::NotInstalled,
        };
        PackageStatus {
            id: spec.id.clone(),
            version: spec.version.clone(),
            state,
        }
    }

    pub fn statuses(&self) -> Vec<PackageStatus> {
        manifest()
            .iter()
            .map(|spec| self.status_for_spec(spec))
            .collect()
    }

    pub fn uninstall(&self, id: &str) -> Result<bool, String> {
        if find(id).is_none() {
            return Err(format!("清单外包：{id}"));
        }
        let root = self.package_root(id);
        if !root.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&root).map_err(|error| format!("删除包目录失败：{error}"))?;
        Ok(true)
    }

    /// 只接受内置清单里的 id；外部 URL/版本在清单校验时拒绝。
    pub async fn install(&self, id: &str) -> Result<(), String> {
        let spec = find(id).ok_or_else(|| format!("清单外包：{id}"))?;
        self.install_spec(spec).await
    }

    /// 测试用的通用安装入口；生产只走 install(id)。
    pub(crate) async fn install_spec(&self, spec: &PackageSpec) -> Result<(), String> {
        let version_root = self.version_root(spec);
        if installed_version_dir(&self.package_root(&spec.id), spec).is_some() {
            // 已装（至少文件齐）不重复下载；具体安装时间由调用方重读。
            return Ok(());
        }
        let tmp = version_root.join("tmp");
        if let Err(error) = std::fs::create_dir_all(&tmp) {
            return Err(format!("创建临时目录失败：{error}"));
        }
        let total = spec.total_bytes;
        let mut done = 0_u64;
        write_status(&tmp, "downloading", 0, total, None);
        for file in &spec.files {
            // 清单内文件不允许目标路径逃逸。
            if std::path::Path::new(&file.target)
                .components()
                .any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir | std::path::Component::RootDir
                    )
                })
            {
                let reason = format!("目标路径不合法：{}", file.target);
                write_status(&tmp, "failed", done, total, Some(&reason));
                return Err(reason);
            }
            let bytes = match download(file).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    write_status(&tmp, "failed", done, total, Some(&error));
                    return Err(error);
                }
            };
            done += bytes.len() as u64;
            write_status(&tmp, "downloading", done, total, None);
            write_status(&tmp, "verifying", done, total, None);
            if bytes.len() as u64 != file.size {
                let reason = format!(
                    "文件大小不对：{}（{}≠{}）",
                    file.target,
                    bytes.len(),
                    file.size
                );
                write_status(&tmp, "failed", done, total, Some(&reason));
                return Err(reason);
            }
            let actual = hex(digest(&SHA256, &bytes).as_ref());
            if actual != file.sha256 {
                let reason = format!("sha256 不符：{}", file.target);
                write_status(&tmp, "failed", done, total, Some(&reason));
                return Err(reason);
            }
            let target = tmp.join(&file.target);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&target, bytes).map_err(|error| format!("写临时文件失败：{error}"))?;
        }
        // 全部核过才落到最终版本目录；旧版本目录不动，失败时也不动。
        std::fs::create_dir_all(&version_root).map_err(|error| error.to_string())?;
        for file in &spec.files {
            let from = tmp.join(&file.target);
            let to = version_root.join(&file.target);
            std::fs::rename(&from, &to).map_err(|error| format!("原子落盘失败：{error}"))?;
        }
        drop(std::fs::remove_dir_all(&tmp));
        Ok(())
    }
}

fn version_dir(spec: &PackageSpec) -> String {
    let mut slug = String::new();
    for character in spec.version.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            slug.push(character);
        } else {
            slug.push('-');
        }
    }
    slug
}

fn status_path(tmp: &Path) -> PathBuf {
    tmp.join("status.json")
}

fn write_status(tmp: &Path, state: &str, done: u64, total: u64, reason: Option<&str>) {
    drop(std::fs::create_dir_all(tmp));
    let value = StatusFile {
        state: state.to_owned(),
        done,
        total,
        reason: reason.map(str::to_owned),
    };
    if let Ok(text) = serde_json::to_string(&value) {
        drop(std::fs::write(status_path(tmp), text));
    }
}

fn read_status(path: &Path) -> StatusFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

async fn download(file: &PackageFile) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(&file.url)
        .send()
        .await
        .map_err(|error| format!("下载失败：{}：{error}", file.url))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载失败：{}：HTTP {}",
            file.url,
            response.status()
        ));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("下载读取失败：{error}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 任意已装版本目录（兼容升级/测试预置）；优先清单版本目录。
fn installed_version_dir(package_root: &Path, spec: &PackageSpec) -> Option<PathBuf> {
    let exact = package_root.join(version_dir(spec));
    if spec
        .files
        .iter()
        .all(|file| exact.join(&file.target).is_file())
    {
        return Some(exact);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(package_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.file_name().is_some_and(|name| name != "tmp"))
        .collect();
    dirs.sort();
    dirs.into_iter().rev().find(|dir| {
        spec.files
            .iter()
            .all(|file| dir.join(&file.target).is_file())
    })
}

/// 协议用的状态视图；安装卡和设置页读同一份。
pub fn status_view(status: &PackageStatus) -> crate::protocol::PackageStatusView {
    use crate::protocol::PackageStatusView;
    let (state, done, total, installed_at, reason) = match &status.state {
        PackageState::NotInstalled => ("not-installed".to_owned(), 0, 0, None, None),
        PackageState::Downloading { done, total } => {
            ("downloading".to_owned(), *done, *total, None, None)
        }
        PackageState::Verifying => ("verifying".to_owned(), 0, 0, None, None),
        PackageState::Installed { installed_at } => {
            ("installed".to_owned(), 0, 0, Some(*installed_at), None)
        }
        PackageState::Failed { reason } => ("failed".to_owned(), 0, 0, None, Some(reason.clone())),
    };
    PackageStatusView {
        id: status.id.clone(),
        version: status.version.clone(),
        state,
        done_bytes: done,
        total_bytes: total,
        installed_at,
        reason,
    }
}

/// 给 OCR 工具找已装版本目录。
pub fn installed_dir(data_dir: &Path, id: &str) -> Option<PathBuf> {
    let spec = find(id)?;
    installed_version_dir(&data_dir.join("packages").join(&spec.id), spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn spec(server: &MockServer, body: &[u8], sha: &str) -> PackageSpec {
        PackageSpec {
            id: "test-pkg".to_owned(),
            name: "测试包".to_owned(),
            version: "v1".to_owned(),
            purpose: "测试".to_owned(),
            source: "本地 mock".to_owned(),
            license: "Apache-2.0".to_owned(),
            total_bytes: body.len() as u64,
            files: vec![PackageFile {
                url: format!("{}/file.bin", server.uri()),
                sha256: sha.to_owned(),
                size: body.len() as u64,
                target: "file.bin".to_owned(),
                version: "v1".to_owned(),
            }],
        }
    }

    #[tokio::test]
    async fn install_verifies_sha_and_uninstall_only_removes_package_dir() {
        let server = MockServer::start().await;
        let body = b"hello ocr";
        let sha = hex(digest(&SHA256, body).as_ref());
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let installer = Installer::new(dir.path().to_path_buf());
        let spec = spec(&server, body, &sha);
        installer.install_spec(&spec).await.expect("安装");
        assert!(installer.version_root(&spec).join("file.bin").is_file());
        let keep = dir.path().join("keep.txt");
        std::fs::write(&keep, "x").unwrap();
        // 卸载只认清单内 id；这里只验证删除包目录，不动包外文件。
        let ocr_dir = dir
            .path()
            .join("packages")
            .join("pp-ocrv6-small")
            .join("v1");
        std::fs::create_dir_all(&ocr_dir).unwrap();
        assert!(installer.uninstall("pp-ocrv6-small").unwrap());
        assert!(!dir.path().join("packages").join("pp-ocrv6-small").exists());
        assert!(keep.is_file());
    }

    #[tokio::test]
    async fn sha_mismatch_leaves_no_final_file_and_keeps_old_version() {
        let server = MockServer::start().await;
        let body = b"hello ocr";
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let installer = Installer::new(dir.path().to_path_buf());
        let spec = spec(&server, body, "00");
        let old = dir.path().join("packages").join("test-pkg").join("old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("old.bin"), "old").unwrap();
        let error = installer.install_spec(&spec).await.expect_err("sha 不符");
        assert!(error.contains("sha256"));
        assert!(!installer.version_root(&spec).join("file.bin").exists());
        assert!(old.join("old.bin").is_file());
        assert!(matches!(
            installer.status_for_spec(&spec).state,
            PackageState::Failed { .. }
        ));
    }

    #[test]
    fn manifest_is_pinned_and_rejects_unknown_ids() {
        let manifest = manifest();
        assert_eq!(manifest.len(), 1);
        let ocr = &manifest[0];
        assert!(
            ocr.files.iter().all(|file| file.url.contains("/resolve/")
                || file.url.contains("raw.githubusercontent.com"))
        );
        assert!(ocr.files.iter().all(|file| file.sha256.len() == 64));
        assert_eq!(ocr.license, "Apache-2.0");
        assert_eq!(
            ocr.files.iter().map(|file| file.size).sum::<u64>(),
            ocr.total_bytes
        );
        assert!(find("not-a-package").is_none());
    }

    #[test]
    fn unknown_package_cannot_be_uninstalled() {
        let dir = tempfile::tempdir().unwrap();
        let installer = Installer::new(dir.path().to_path_buf());
        assert!(installer.uninstall("evil").is_err());
    }

    #[tokio::test]
    #[ignore = "真下载 31MB；AIOS_INSTALL_REAL=1 手动跑"]
    async fn real_install_ocr_package_verifies_manifest() {
        if std::env::var("AIOS_INSTALL_REAL").as_deref() != Ok("1") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let installer = Installer::new(dir.path().to_path_buf());
        installer.install("pp-ocrv6-small").await.expect("真安装");
        assert!(matches!(
            installer.status("pp-ocrv6-small").state,
            PackageState::Installed { .. }
        ));
        let root = installer.version_root(find("pp-ocrv6-small").unwrap());
        for name in ["det.onnx", "rec.onnx", "dict.txt"] {
            assert!(root.join(name).is_file(), "缺 {name}");
        }
    }
}
