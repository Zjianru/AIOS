//! T006.16b：Kind 根会话的找回/新建与状态文件（D44、docs/02-Kind与协作.md 第 1 节、strategy §4）。
//!
//! 一个用户只有一条 Kind 根会话：第一次启动建它，把会话 id 记在数据目录的状态文件里；
//! 以后启动只找回它。**记过 id 但会话日志缺失、或状态文件损坏，就报错，不静默新建**
//! （06 篇恢复链：缺失、损坏、身份冲突都明确报告）。
//!
//! 状态文件原子写：先写同目录临时文件、fsync、rename 覆盖；半写不会破坏已提交的状态。

use std::io::Write;
use std::path::{Path, PathBuf};

use agent::{Agent, AgentConfig, AgentError, AgentOpenOptions};
use model::DeepSeekConfig;
use serde::{Deserialize, Serialize};
use session::SessionId;

/// 根会话状态文件名（数据目录下）。
pub const ROOT_STATE_FILE: &str = "kind-root.json";
/// 状态文件 schema 版本（AIOS 自己的；与会话日志 FORMAT_VERSION 无关）。
const ROOT_STATE_VERSION: u64 = 1;

/// 状态文件内容：只记根会话的身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RootRecord {
    version: u64,
    #[serde(rename = "sessionId")]
    session_id: String,
}

/// 根会话四种状态里的成功两态（失败两态走 RootError）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootStatus {
    /// 第一次启动：没有状态文件，新建会话并记录。
    Created,
    /// 已有状态文件且会话日志在：找回同一条根会话。
    Restored,
}

/// 根会话错误：记过但缺失 / 状态文件损坏 / 底层错误。
#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("根会话状态文件损坏：{path}（{reason}）")]
    StateCorrupt {
        path: String,
        reason: String,
        line: Option<u64>,
    },
    #[error("根会话记录为 {session_id}，但会话日志缺失：{path}")]
    RecordedSessionMissing { session_id: String, path: String },
    #[error("根会话 {session_id} 已被其他写者占用（session/writer-held）")]
    WriterHeld { session_id: String },
}

/// 打开/新建根会话的输入：与 Agent::open 一致，外加根会话状态文件所在的数据目录。
#[derive(Clone)]
pub struct RootSessionOptions {
    pub data_dir: PathBuf,
    pub cwd: PathBuf,
    pub model: DeepSeekConfig,
    pub config: AgentConfig,
    /// T008.2：凭证来源（服务传 Keyring 适配器）；None = 没有来源，组装会返回结构化 MissingCredential。
    pub credentials: Option<std::sync::Arc<dyn model::CredentialSource>>,
    /// T008.3：钥匙文件位置；设置方法每次操作重新打开 Keyring。
    pub identity: Option<PathBuf>,
    /// T008.3b：每次打开/recheck 根会话时重新解析模型配置（首次使用后 Keyring 才有连接）。
    pub model_factory: Option<ModelFactory>,
    /// T011.4：打开会话后按 id 造「追加之后」的钩子（监控 ledger）；None = 不记。
    pub append_hook_factory: Option<session::AppendHookFactory>,
    /// F1：Keyring 变化时刷新监控脱敏用的 key 列表；None = 不刷新。
    pub keyring_changed: Option<KeyringChanged>,
}

/// T008.3b/T012.7：按会话的模型选择解析 (连接 id, 模型配置, 凭证来源)。
/// `selection` = 会话日志里的选择（重开恢复）；None = 没有会话选择（新会话，实现方用
/// settings 的上次选择/默认连接）。
pub type ModelFactory = std::sync::Arc<
    dyn Fn(
            Option<&session::ModelSelection>,
        ) -> Result<
            (
                String,
                DeepSeekConfig,
                Option<std::sync::Arc<dyn model::CredentialSource>>,
            ),
            String,
        > + Send
        + Sync,
>;

/// F1：Keyring 变化（启动、设置页保存/删除/撤销、搬迁/重开/重启、recheck）时通知装配层，
/// 由 cli 把当前持有的 key 刷新给监控脱敏；service 不引用监控类型。
pub type KeyringChanged = std::sync::Arc<dyn Fn(&credentials::Keyring) + Send + Sync>;

impl std::fmt::Debug for RootSessionOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RootSessionOptions")
            .field("data_dir", &self.data_dir)
            .field("cwd", &self.cwd)
            .field("model", &self.model)
            .field("config", &self.config)
            .field(
                "append_hook_factory",
                &self.append_hook_factory.as_ref().map(|_| "<factory>"),
            )
            .field(
                "keyring_changed",
                &self.keyring_changed.as_ref().map(|_| "<hook>"),
            )
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "<source>"),
            )
            .finish()
    }
}

/// 根会话打开结果。
pub struct RootSession {
    pub agent: Agent,
    pub session_id: SessionId,
    pub status: RootStatus,
    /// 重启找回时是否领取了 pending inbox（DoD6 的服务重启路径；新建时总是 false）。
    pub claimed_pending: bool,
}

impl RootSession {
    /// 状态文件路径（测试/诊断用）。
    pub fn state_path(data_dir: &Path) -> PathBuf {
        data_dir.join(ROOT_STATE_FILE)
    }
}

/// 找回或新建 Kind 根会话（T006.16b）：四态见 docs/02 篇第 1 节与 06 篇恢复链。
pub async fn open_root_session(options: RootSessionOptions) -> Result<RootSession, RootError> {
    let state_path = RootSession::state_path(&options.data_dir);
    match std::fs::read(&state_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&options.data_dir)?;
            let agent = Agent::open(AgentOpenOptions {
                data_dir: options.data_dir.clone(),
                cwd: options.cwd,
                resume: None,
                model: options.model,
                config: options.config,
                credentials: options.credentials.clone(),
                append_hook_factory: options.append_hook_factory.clone(),
            })
            .await?;
            let record = RootRecord {
                version: ROOT_STATE_VERSION,
                session_id: agent.id().as_str().to_owned(),
            };
            write_root_state_atomic(&options.data_dir, &state_path, &record)?;
            Ok(RootSession {
                session_id: agent.id().clone(),
                agent,
                status: RootStatus::Created,
                claimed_pending: false,
            })
        }
        Err(error) => Err(RootError::Io(error)),
        Ok(bytes) => {
            let record: RootRecord =
                serde_json::from_slice(&bytes).map_err(|error| RootError::StateCorrupt {
                    path: state_path.display().to_string(),
                    reason: error.to_string(),
                    line: Some(error.line() as u64),
                })?;
            if record.version != ROOT_STATE_VERSION {
                return Err(RootError::StateCorrupt {
                    path: state_path.display().to_string(),
                    reason: format!("未知状态文件版本 {}", record.version),
                    line: None,
                });
            }
            let id =
                SessionId::parse(&record.session_id).map_err(|error| RootError::StateCorrupt {
                    path: state_path.display().to_string(),
                    reason: error.to_string(),
                    line: None,
                })?;
            let log_path = options
                .data_dir
                .join("sessions")
                .join(format!("{id}.jsonl"));
            if !log_path.exists() {
                return Err(RootError::RecordedSessionMissing {
                    session_id: record.session_id.clone(),
                    path: log_path.display().to_string(),
                });
            }
            let agent = Agent::open(AgentOpenOptions {
                data_dir: options.data_dir.clone(),
                cwd: options.cwd,
                resume: Some(id.clone()),
                model: options.model,
                config: options.config,
                credentials: options.credentials.clone(),
                append_hook_factory: options.append_hook_factory.clone(),
            })
            .await
            .map_err(|error| match &error {
                // T006.18：服务/别的进程占着写锁时给稳定错误码语义（DSH session/writer-held）。
                AgentError::Store(session::SessionError::AlreadyOwned { .. }) => {
                    RootError::WriterHeld {
                        session_id: id.as_str().to_owned(),
                    }
                }
                _ => RootError::Agent(error),
            })?;
            // DoD6：重启后 pending inbox 自动领取转 turn（2a 的 resume 只恢复、不唤醒）；
            // 领取恰好一次：队列消费后日志里就不会再有 pending。
            let claimed_pending = agent.claim_pending();
            Ok(RootSession {
                session_id: id,
                agent,
                status: RootStatus::Restored,
                claimed_pending,
            })
        }
    }
}

/// T012.7：从根会话日志恢复模型选择（重开会话还是它）。
///
/// 没有根状态文件、状态文件坏了、会话日志缺失或没有 session/model 事件时返回 None
/// （交给实现方按 settings 的上次选择解析）；这里不报错——严格恢复链错误由
/// open_root_session 负责。
pub fn session_model_selection(data_dir: &Path) -> Option<session::ModelSelection> {
    let bytes = std::fs::read(RootSession::state_path(data_dir)).ok()?;
    let record: RootRecord = serde_json::from_slice(&bytes).ok()?;
    if record.version != ROOT_STATE_VERSION {
        return None;
    }
    let id = SessionId::parse(&record.session_id).ok()?;
    let snapshot = session::SessionStore::open_read_only(data_dir, &id).ok()?;
    session::last_model_selection(snapshot.events())
}

/// 原子写状态文件：同目录临时文件 → fsync → rename 覆盖 → 目录 fsync（best effort）。
fn write_root_state_atomic(
    data_dir: &Path,
    path: &Path,
    record: &RootRecord,
) -> Result<(), RootError> {
    let json = serde_json::to_vec(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    // rename 的持久性要靠目录 fsync；失败只影响掉电场景，不破坏当前进程。
    if let Ok(dir) = std::fs::File::open(data_dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}
