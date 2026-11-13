//! T006.16b：Kind 根会话的找回/新建与状态文件（D44、docs/02-Kind与协作.md 第 1 节、strategy §4）。
//!
//! 一个用户只有一条 Kind 根会话：第一次启动建它，把会话 id 记在数据目录的状态文件里；
//! 以后启动只找回它。**记过 id 但会话日志缺失、或状态文件损坏，就报错，不静默新建**
//! （06 篇恢复链：缺失、损坏、身份冲突都明确报告）。
//!
//! 状态文件原子写走 credentials::atomic（临时文件 + fsync + rename + 目录 fsync）；
//! 半写不会破坏已提交的状态。

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
    /// 服务端登记的动作（确认卡背后能做的事）；空表时提议工具永远不开出。
    pub actions: std::sync::Arc<crate::actions::ActionRegistry>,
    /// 仅开发/端到端测试用：覆盖 Exa 的根地址（默认 https://api.exa.ai）。
    /// 生产不设；验证 Exa key 的请求走它。
    pub exa_base_url: Option<String>,
    /// AIOS 自己的监听地址；web::serve 绑定后填入。web_fetch 读它拒绝读自己。
    pub web_listen: Option<std::sync::Arc<std::sync::OnceLock<std::net::SocketAddr>>>,
    /// T024.3：任务执行者（服务装配；测试可借它暂停效果泵）。None = 不开任务工具。
    #[doc(hidden)]
    pub task_runtime: Option<std::sync::Arc<crate::executor::TaskRuntime>>,
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
            .field("actions", &self.actions.skills())
            .field("exa_base_url", &self.exa_base_url)
            .field(
                "web_listen",
                &self
                    .web_listen
                    .as_ref()
                    .and_then(|cell| cell.get().copied()),
            )
            .field(
                "task_runtime",
                &self.task_runtime.as_ref().map(|_| "<runtime>"),
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

/// Kind 的工具面（D63）：只读、skill、todo、提问、画布、查询、确认卡、联网。
///
/// 写、改、跑命令和三件后台任务不在里面；这些活派给 ThinAgent。任务工具看
/// `task_runtime` 有没有装配，由 `kind_tool_policy` 追加。
const KIND_TOOLS: [&str; 11] = [
    "read",
    "glob",
    "grep",
    "skill",
    "todo_write",
    "ask_user_question",
    "canvas",
    "aios_query",
    "propose_action",
    "web_search",
    "web_fetch",
];

/// Kind 根会话的派任务说明（D20、D63）：照 DSH superagent 协调者的几条，按 AIOS 改名。
///
/// 只在装了任务执行者时追加到系统提示末尾；没有执行者时不提 Kind 没有的工具。
const KIND_POLICY: &str = concat!(
    "You are AIOS's coordinator: you talk with the user, verify facts, and hand work to ThinAgent. ThinAgent investigates and implements.",
    "\nWhen the user wants a file changed (even a single typo), a command run, or an investigation that reads more than one or two files, call task_accept to dispatch a task; then reply in the user's language with what you dispatched and the result you expect.",
    "\nWrite the task's requirement in the user's own words, with no implementation plan or expanded scope; write its dod as the criteria for judging that requirement met.",
    "\nWrite the task's title as a short card label of at most 20 characters (Chinese characters count as one); it is the card title, so do not write a long sentence.",
    "\nIf one message contains several unrelated things, dispatch them as separate tasks, one task per thing; do not split one thing into several.",
    "\nResults arrive as task_report: do not wait or poll; finish the turn once the work is dispatched and the reply is sent.",
    "\nRead only until you can answer the user or state the task clearly (for example, confirm that a file exists); do not investigate on ThinAgent's behalf.",
    "\nWhen ThinAgent is blocked or hands back, relay briefly what it is missing or asking; the user decides, so do not make the decision for them.",
    "\nBefore dispatching a write task, stopping a task, or changing its write scope, load the aios-tasks skill first.",
    "\nWhen the user's request has steps that must happen in order (a later step needs an earlier step's result), call task_graph_propose once with the subtasks and their after links, then call propose_action once with action start_task_graph and the graph id it returned; the user confirms on the card. Sub-tasks whose upstream has a ready result start automatically: do not dispatch them again.",
    "\nDispatch tasks that are independent of each other separately with task_accept; only ordered work belongs in a task graph.",
);

/// Kind 的构造期工具策略：上面那份名单，有任务执行者时再加四个任务工具。
///
/// `no_wait` 照 T024.3 留着；Kind 已经没有 bash 和后台任务，这条策略现在基本用不到。
fn kind_tool_policy(options: &RootSessionOptions) -> agent::ToolPolicy {
    let mut only: Vec<String> = KIND_TOOLS.iter().map(|name| (*name).to_owned()).collect();
    if options.task_runtime.is_some() {
        only.extend(
            [
                "task_accept",
                "task_status",
                "task_stop",
                "task_update",
                "task_graph_propose",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    agent::ToolPolicy {
        only: Some(only),
        no_wait: true,
    }
}

/// 找回或新建 Kind 根会话（T006.16b）：四态见 docs/02 篇第 1 节与 06 篇恢复链。
pub async fn open_root_session(options: RootSessionOptions) -> Result<RootSession, RootError> {
    let state_path = RootSession::state_path(&options.data_dir);
    // 根会话独有的工具：`aios_query` 读服务里已存的数据，`propose_action` 出确认卡，
    // `web_search`/`web_fetch` 联网（要 Exa 的 key 且加载过 `web`），任务工具
    // `task_accept`/`task_status` 派只读调查。CLI 的对话不装它们。
    let mut extra_tools = vec![
        std::sync::Arc::new(crate::aios_query::QueryTool::new(
            options.data_dir.clone(),
            options.identity.clone(),
        )) as std::sync::Arc<dyn tools::Tool>,
        std::sync::Arc::new(crate::propose::ProposeActionTool::new(
            std::sync::Arc::clone(&options.actions),
            options.data_dir.clone(),
            options.identity.clone(),
        )),
    ];
    extra_tools.extend(web_extra_tools(&options));
    if let Some(runtime) = &options.task_runtime {
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskAcceptTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskStatusTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskStopTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskUpdateTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(
            crate::executor::TaskGraphProposeTool::new(std::sync::Arc::clone(runtime)),
        ));
    }
    let tool_policy = kind_tool_policy(&options);
    let policy_prompt = options
        .task_runtime
        .as_ref()
        .map(|_| KIND_POLICY.to_owned());
    // 工具凭证的事实：每步只读检查 Keyring（不建任何文件）；读不出来按没存，
    // 设置页另外报读不出的原因。ThinAgent 用同一份来源。
    let mut config = options.config.clone();
    config.tool_keys = tool_key_source(&options);
    match std::fs::read(&state_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&options.data_dir)?;
            let agent = Agent::open(AgentOpenOptions {
                data_dir: options.data_dir.clone(),
                cwd: options.cwd,
                resume: None,
                model: options.model,
                config: config.clone(),
                credentials: options.credentials.clone(),
                append_hook_factory: options.append_hook_factory.clone(),
                extra_tools: extra_tools.clone(),
                // Kind 的工具面：写/改/跑命令交给 ThinAgent；不原地等（D63、T024.3）。
                tool_policy: tool_policy.clone(),
                policy_prompt: policy_prompt.clone(),
            })
            .await?;
            let record = RootRecord {
                version: ROOT_STATE_VERSION,
                session_id: agent.id().as_str().to_owned(),
            };
            write_root_state_atomic(&state_path, &record)?;
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
                config,
                credentials: options.credentials.clone(),
                append_hook_factory: options.append_hook_factory.clone(),
                extra_tools,
                // Kind 的工具面：写/改/跑命令交给 ThinAgent；不原地等（D63、T024.3）。
                tool_policy: tool_policy.clone(),
                policy_prompt: policy_prompt.clone(),
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

/// 原子写状态文件：临时文件 + fsync + rename + 目录 fsync，失败向上返回（credentials::atomic）。
fn write_root_state_atomic(path: &Path, record: &RootRecord) -> Result<(), RootError> {
    let mut json = serde_json::to_vec(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    json.push(b'\n');
    credentials::atomic::write(path, &json, credentials::atomic::TargetKind::Shared)?;
    Ok(())
}

/// 根会话和 ThinAgent 共用的联网工具（两件都挂 AfterSkillWithKey，构造方式只此一处）。
pub(crate) fn web_extra_tools(
    options: &RootSessionOptions,
) -> Vec<std::sync::Arc<dyn tools::Tool>> {
    let web_search_config = crate::web::WebSearchConfig {
        data_dir: options.data_dir.clone(),
        identity: options.identity.clone(),
        base_url: options
            .exa_base_url
            .clone()
            .unwrap_or_else(|| crate::EXA_DEFAULT_BASE_URL.to_owned()),
    };
    vec![
        std::sync::Arc::new(crate::web::WebSearchTool::new(web_search_config)),
        std::sync::Arc::new(crate::web::WebFetchTool::new(crate::web::WebFetchConfig {
            data_dir: options.data_dir.clone(),
            identity: options.identity.clone(),
            self_addr: options.web_listen.clone(),
            http: model::http::HttpEnv::from_process(),
            timeout: crate::web::WEB_FETCH_TIMEOUT,
        })),
    ]
}

/// 工具凭证的求值面：每步只读 Keyring；读不出来按没存（设置页另报原因）。
pub(crate) fn tool_key_source(options: &RootSessionOptions) -> tools::ToolKeySource {
    let data_dir = options.data_dir.clone();
    let identity = options.identity.clone();
    tools::ToolKeySource::new(std::sync::Arc::new(move || {
        let mut keys = std::collections::BTreeSet::new();
        if credentials::Keyring::open_read_only(&data_dir, identity.as_deref())
            .is_ok_and(|keyring| keyring.exa_key_saved())
        {
            keys.insert(tools::ToolKey::Exa);
        }
        keys
    }))
}

/// T048：Kind 系统提示也要直接告诉模型派单标题的字数上限（卡片标题，20 字以内，中文字符按一字计）。
#[cfg(test)]
mod t048_title_limit_policy_tests {
    use super::KIND_POLICY;

    #[test]
    fn kind_policy_states_the_task_title_limit() {
        assert!(
            KIND_POLICY.contains("at most 20 characters"),
            "KIND_POLICY 要写明确上限：{KIND_POLICY}"
        );
        assert!(
            KIND_POLICY.contains("card label"),
            "KIND_POLICY 要说明它是卡片标题：{KIND_POLICY}"
        );
    }
}
