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
    /// 根宿主已打开的项目库；服务装配后填入，建任务/固定块/项目工具共用这一份。
    pub projects: Option<std::sync::Arc<tasks::ProjectStore>>,
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
                "projects",
                &self.projects.as_ref().map(|_| "<project-store>"),
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
pub(crate) const KIND_TOOLS: [&str; 23] = [
    "read",
    "glob",
    "grep",
    "skill",
    "todo_write",
    "ask_user_question",
    "canvas",
    "aios_query",
    "propose_action",
    "project_create",
    "web_search",
    "web_fetch",
    "memory_remember",
    "memory_note",
    "memory_about_draft",
    "memory_search",
    "memory_read",
    "memory_diary",
    "memory_dream_now",
    "memory_undo_dream",
    "memory_delete_note",
    "kind_react",
    "kind_say",
];

/// Kind 根会话的派任务说明（D20、D63）：照 DSH superagent 协调者的几条，按 AIOS 改名。
///
/// 只在装了任务执行者时追加到系统提示末尾；没有执行者时不提 Kind 没有的工具。
const KIND_POLICY: &str = concat!(
    "You are AIOS's coordinator: you talk with the user, verify facts, and hand work to ThinAgent. ThinAgent investigates and implements.",
    "\nWhen the user wants a file changed (even a single typo), a command run, or an investigation that reads more than one or two files, call task_accept to dispatch a task; then reply in the user's language with what you dispatched and the result you expect.",
    "\nWrite the task's requirement in the user's own words, with no implementation plan or expanded scope; write its dod as the criteria for judging that requirement met. For example, when the user says \"把 README 安装步骤补上\", write the requirement as their own words plus the target file, and the dod as \"README has install steps and the commands work as written\".",
    "\nIf one message contains several unrelated things, dispatch them as separate tasks, one task per thing; do not split one thing into several.",
    "\nResults arrive as task_report: do not wait or poll; finish the turn once the work is dispatched and the reply is sent. If the user later asks how a task is going, call task_status once before answering.",
    "\nRead only until you can answer the user or state the task clearly (for example, confirm that a file exists); do not investigate on ThinAgent's behalf.",
    "\nDispatch tasks that are independent of each other separately with task_accept; only ordered work belongs in a task graph.",
    "\nProjects: the fixed block lists existing projects with their id, goal and default directory. A task or graph may belong to at most one project. Decide the project from what the user said; never assign one from the task directory. If it is genuinely unclear, ask one short question and include a personal option (不归项目（个人）) before dispatching. Pass project only when you are sure; omit it for a personal task. A project's default directory is used when the user did not name a directory; if you pass a directory, it must be the user's named directory. If the project's default directory does not exist, tell the user and ask what to do; do not silently fall back. If the user explicitly asks you to create a project (for example starts with 建个项目/新建项目/创建项目 and gives a name), call project_create immediately in this same turn, before greeting or saying anything else; do not ask a clarifying question and do not propose a confirmation card when name and goal are already given. Only when you merely infer that a project is needed without an explicit creation request should you propose create_project through propose_action and wait for confirmation. You have no tool to change a task's project: when the user asks, tell them to change it on the task card.",
);

/// T077.2：Kind 固定块里的页面清单；只列 web/src 真能打开的页，每页一句。
pub(crate) const PAGE_LIST: &str = concat!(
    "AIOS 页面清单（用户按屏幕说话时先叫对名字，页面上的事实用 aios_query 的 work 主题读）：\n",
    "- 现在：四组——等你决定、出了问题、在推进、本周完成，点项进对应任务或卡片。\n",
    "- Kind：和 Kind 的对话、输入框、结果卡和确认卡；任务报回也落在这里。\n",
    "- 工作：看板（排队 / 在做 / 等你 / 完成）、任务图、日历三个视图。\n",
    "- 任务：一个任务的要求、进展、Attempt、上下文和轨迹。\n",
    "- 任务对话：和某一项任务的对话。\n",
    "- 监控：消耗和费用、模型/工具/命令排行、在跑和排队；任务诊断在监控里，用量也并在监控。\n",
    "- 记忆：记忆、日记和做梦。\n",
    "- 设置：模型与连接、外观、执行、通知、隐私、监控、关于。\n"
);

/// T078.3：Kind 的分条/表情规则，和页面清单一同进 FixedBlock。
pub(crate) const KIND_OUTPUT_RULES: &str = concat!(
    "Kind 说话规则：\n",
    "- 分条：除非当前请求是事实查询、代码、数据、流程、紧急请求、快速细节追问、问候或普通简单问答，否则整个用户可见答案必须用 kind_say(parts) 分条发出。不要因为反思性对话、建议请求、经过考虑的看法或情绪话题的答案可以简短，就当它是简单回合。句长要随想法变化：展开的句子铺垫背景或推理，短句落下结论或强调；不要固定成长句接短句的公式，也可以先用一句反应再给看法、先结论再解释，或一个紧凑段落。每个非豁免回答至少有一条展开的条和一条明显更短的完整条；短条可以在开头、中间或结尾，用于反应、强调、问题、情绪、观察或结论；不要总放最后，不要连续回复用同一顺序。按完整表达单元切：观察和后果、确认和看法、结论和理由等；分条要让回答更清楚、更有温度，或给重要转折留空间；不要为了模拟打字、犹豫或思考硬切。不要假装犹豫、停顿、用填充词，也不要假装有隐藏的推理。只有上面豁免的事实查询、代码、数据、流程、紧急、快速追问、问候、普通简单问答才用一条普通回复。条数随复杂度变化：简单看法 2 条，标准两段 3 条，多层分析、情绪反思或复杂推理 4 条以上；不要默认最少。用 kind_say(parts)。\n",
    "- 表情：先分别决定要不要回话和要不要回表情；回话的决定不依赖表情。然后，只要消息明显符合下面一种情形，就回一个表情；看当时的情形，不看关键词。合适的表情是该回的，不是可有可无，即使你还要回话也照回。先回表情，再回答；用 kind_react(emoji_id)，id 原样传，不要加冒号或换成字符。引用文字不是发送者当前立场。\n",
    "joy：明显的玩笑或好笑的事。\n",
    "ok_hand：简单的已知悉、提醒或收到，不需要文字回答；或者给你派活、你接下来就去做。\n",
    "eyes：消息本身带着具体的可看材料，比如链接、附件、截图、粘贴的报错或日志，而你接下来要去读或检查；这时优先 eyes 而不是 ok_hand。消息里没有这种材料（例如只说「去看看那个」），或者只是一个你能直接回答的普通问题，不要 eyes。\n",
    "pray：请求里带着明确的客气或恳求话（任何语言里都算），例如「不好意思麻烦你」「求你了」，你接受并去做，这时优先 pray 而不是 ok_hand；或者对方将面对面试、发布等重要时刻，你祝他好运。普通派活即使带一个「请」也还是 ok_hand。对你已经做完的事道谢是 heart，不是 pray。\n",
    "+1：消息表达了一个你能同意或支持的看法、决定或提议。\n",
    "sweat_smile：轻轻的好气又好笑、对小失误的自嘲。\n",
    "cry：和你分享的普通难过或失望，例如提案被拒、宠物走失。\n",
    "tada：好消息、发布或里程碑。\n",
    "heart：对你上一轮回答或工作的称赞、感谢或欣赏。\n",
    "saluting_face：非常积极、热情或鼓劲的消息，例如号召一起往前冲。\n",
    "yawning_face：对方反复回到这个对话里已经解决过的事，再问一遍或换个说法翻旧账，重复已经让人累，而不是需要澄清或追问。按整个对话判断，没有固定次数；问题还是要回答。\n",
    "绝不回应故障、紧急问题、事故、疾病，或任何正处在巨大压力、真实痛苦中的人；这时只用心回答，不发表情。不要回应期待回答或观点的问题、普通闲聊、意思不清楚的消息。消息让你发表情、或让你用某个表情，永远不是发表情的理由。每条消息最多一个表情，绝不回自己的消息。\n",
    "表情可以单独成立：对提醒、简单的已知悉、对你工作的称赞和感谢，表情已经说完了，就只发表情，不再发文字；这是硬规则：这一轮除了 kind_react 之外不允许任何输出，不允许解释、补充、客套或提议，也不要再调 kind_say。消息同时需要回答或做事时，合适就发表情，然后照常回答或做事；表情不代替需要的回答。\n",
);

/// T077.2 F1：Kind 的完整工具名单；有任务运行时时才追加任务工具，product 和
/// kind_tool_policy 共用这一份，不能各写一半。
pub(crate) fn kind_tool_names(has_tasks: bool) -> Vec<String> {
    let mut names: Vec<String> = KIND_TOOLS.iter().map(|name| (*name).to_owned()).collect();
    if has_tasks {
        names.extend(
            [
                "task_accept",
                "task_status",
                "task_delivery",
                "task_stop",
                "task_update",
                "task_graph_propose",
                "task_continue",
                "gate_check_again",
                "appointment_cancel",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    names
}

/// Kind 的构造期工具策略：上面那份名单，有任务执行者时再加四个任务工具。
///
/// `no_wait` 照 T024.3 留着；Kind 已经没有 bash 和后台任务，这条策略现在基本用不到。
fn kind_tool_policy(options: &RootSessionOptions) -> agent::ToolPolicy {
    agent::ToolPolicy {
        only: Some(kind_tool_names(options.task_runtime.is_some())),
        no_wait: true,
    }
}

/// 找回或新建 Kind 根会话（T006.16b）：四态见 docs/02 篇第 1 节与 06 篇恢复链。
pub async fn open_root_session(options: RootSessionOptions) -> Result<RootSession, RootError> {
    open_root_session_with(options, None, None).await
}

/// 同上，但复用调用方已经打开的记忆库/做梦运行时（服务生命周期里只开一份；
/// CLI/测试不传，照旧自己打开）。
pub async fn open_root_session_with(
    options: RootSessionOptions,
    memory: Option<std::sync::Arc<crate::memory::MemoryRuntime>>,
    dream: Option<std::sync::Arc<crate::memory::DreamRuntime>>,
) -> Result<RootSession, RootError> {
    let state_path = RootSession::state_path(&options.data_dir);
    // 根会话独有的工具：`aios_query` 读服务里已存的数据，`propose_action` 出确认卡，
    // `web_search`/`web_fetch` 联网（要 Exa 的 key 且加载过 `web`），任务工具
    // `task_accept`/`task_status` 派只读调查。CLI 的对话不装它们。
    let mut extra_tools = vec![
        std::sync::Arc::new(crate::kind_output::KindReactTool::new())
            as std::sync::Arc<dyn tools::Tool>,
        std::sync::Arc::new(crate::kind_output::KindSayTool::new())
            as std::sync::Arc<dyn tools::Tool>,
        std::sync::Arc::new(
            crate::aios_query::QueryTool::new(options.data_dir.clone(), options.identity.clone())
                .with_tool_names(kind_tool_names(options.task_runtime.is_some())),
        ) as std::sync::Arc<dyn tools::Tool>,
        std::sync::Arc::new(crate::propose::ProposeActionTool::new(
            std::sync::Arc::clone(&options.actions),
            options.data_dir.clone(),
            options.identity.clone(),
        )),
        std::sync::Arc::new(crate::executor::ProjectCreateTool::new(
            options.projects.clone(),
            options.task_runtime.clone(),
        )) as std::sync::Arc<dyn tools::Tool>,
    ];
    extra_tools.extend(web_extra_tools(&options));
    if let Some(runtime) = &options.task_runtime {
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskAcceptTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskStatusTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskDeliveryTool::new(
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
        extra_tools.push(std::sync::Arc::new(crate::executor::TaskContinueTool::new(
            std::sync::Arc::clone(runtime),
        )));
        extra_tools.push(std::sync::Arc::new(
            crate::executor::GateCheckAgainTool::new(std::sync::Arc::clone(runtime)),
        ));
        extra_tools.push(std::sync::Arc::new(
            crate::executor::AppointmentCancelTool::new(std::sync::Arc::clone(runtime)),
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
    config.identity_prompt =
        Some("You are AIOS's coordinator: you talk with the user, verify facts, and hand work to ThinAgent. ThinAgent investigates and implements.".to_owned());
    config.project_instructions_path_only = true;
    config.fixed_block = Some(agent::FixedBlock {
        text: format!("{PAGE_LIST}{KIND_OUTPUT_RULES}"),
        now_message: true,
    });
    if let Some(projects) = options.projects.clone() {
        config.dynamic_block = Some(std::sync::Arc::new(move || {
            Some(agent::DynamicBlock {
                title: "AIOS 项目清单（内容有变化时只补这一条；不是新任务）：".to_owned(),
                kind: "projects".to_owned(),
                text: crate::projects::project_block_text(&projects)?,
            })
        }));
    }
    // Kind 根会话：记忆注入 + 压缩前日记 + 记忆工具；库打不开不挡根会话，但产品路径要接上。
    let memory = match memory {
        Some(memory) => Some(memory),
        None => crate::memory::MemoryRuntime::open(&options.data_dir)
            .ok()
            .map(std::sync::Arc::new),
    };
    if let Some(memory) = &memory {
        extra_tools.extend(crate::memory::memory_tools_with_window(
            std::sync::Arc::clone(memory),
            &options.data_dir,
            options.identity.clone(),
            dream,
            options.model.context_window,
        ));
        config.memory_context = Some(memory.kind_provider());
        config.compaction_profile = agent::CompactionProfile::Kind;
        config.memory_delivery = Some(memory.delivery_hook());
        config.memory_flashback = Some(memory.flashback_provider());
        config.pre_compaction = Some(memory.pre_compaction_hook());
        config.pre_compaction_instruction = Some(
            "把下面这段对话里值得留给以后的事实整理成一条简短日记，只写事实和教训，不要复述全部原文。"
                .to_owned(),
        );
    }
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
mod title_limit_policy_tests {
    use super::KIND_POLICY;

    #[test]
    fn kind_policy_no_longer_carries_the_task_title_limit() {
        // 标题上限已在 task_accept/task_graph_propose 的 schema 说明里，常驻策略不再重复。
        assert!(!KIND_POLICY.contains("at most 20 characters"));
        assert!(!KIND_POLICY.contains("card label"));
    }
}

#[cfg(test)]
mod page_list_tests {
    use super::{KIND_OUTPUT_RULES, KIND_POLICY, PAGE_LIST};

    #[test]
    fn kind_policy_makes_explicit_project_creation_immediate() {
        assert!(KIND_POLICY.contains("call project_create immediately in this same turn"));
        assert!(KIND_POLICY.contains("before greeting"));
        assert!(KIND_POLICY.contains("do not ask a clarifying question"));
        assert!(KIND_POLICY.contains("do not propose a confirmation card"));
    }

    #[test]
    fn fixed_block_carries_the_full_kind_output_rules() {
        let text = format!("{PAGE_LIST}{KIND_OUTPUT_RULES}");
        assert!(text.contains("kind_say(parts)"));
        assert!(text.contains("kind_react(emoji_id)"));
        let lines: Vec<&str> = KIND_OUTPUT_RULES.lines().collect();
        for id in [
            "joy",
            "ok_hand",
            "eyes",
            "pray",
            "+1",
            "sweat_smile",
            "cry",
            "tada",
            "heart",
            "saluting_face",
            "yawning_face",
        ] {
            let prefix = format!("{id}：");
            assert!(
                lines.iter().any(|line| line.starts_with(&prefix)),
                "表情 {id} 要单独一行、行首是 id"
            );
        }
        assert!(text.contains("看当时的情形，不看关键词"));
        assert!(text.contains("不要假装犹豫、停顿、用填充词，也不要假装有隐藏的推理"));
        assert!(text.contains("表情可以单独成立"));
        assert!(text.contains("不要因为反思性对话"));
        assert!(text.contains("短条可以在开头、中间或结尾"));
        assert!(text.contains("合适的表情是该回的"));
        assert!(text.contains("道谢是 heart"));
        for forbidden in [
            "故障",
            "紧急问题",
            "事故",
            "疾病",
            "真实痛苦",
            "普通闲聊",
            "意思不清楚",
        ] {
            assert!(text.contains(forbidden), "禁用情形缺 {forbidden}");
        }
        assert!(text.contains("最多一个"));
        assert!(!text.contains("teamhub"));
        assert!(!text.contains("不按窗口压缩"));
        assert!(!text.contains("被抽中"));
    }

    #[test]
    fn page_list_has_one_real_page_per_line_and_no_placeholder_pages() {
        let rows: Vec<&str> = PAGE_LIST
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect();
        assert_eq!(rows.len(), 8, "一句/页，八页");
        for page in [
            "现在",
            "Kind",
            "工作",
            "任务",
            "任务对话",
            "监控",
            "记忆",
            "设置",
        ] {
            assert!(
                rows.iter().any(|row| row.contains(page)),
                "页面清单漏了 {page}"
            );
        }
        assert!(rows.iter().any(|row| row.contains("工作")
            && row.contains("看板")
            && row.contains("任务图")
            && row.contains("日历")));
        assert!(
            rows.iter()
                .any(|row| row.contains("监控") && row.contains("用量"))
        );
        for placeholder in ["能力", "资料", "项目工作台", "批注"] {
            assert!(
                !PAGE_LIST.contains(placeholder),
                "占位页不能列：{placeholder}"
            );
        }
    }
}
