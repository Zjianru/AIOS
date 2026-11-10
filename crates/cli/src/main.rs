//! 开发用 CLI：aios-dev chat / aios-dev run。

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use agent::{
    Agent, AgentConfig, AgentEvent, AgentOpenOptions, CompactionOutcome, Delivery, JobStatus,
    TurnOutcome,
};
use clap::{Args, Parser, Subcommand};
use model::{DEFAULT_MAX_TOKENS, DeepSeekConfig, Effort, UserMessage};
use monitor::{Channel, Monitor, Record, Severity};
use service::{RootHost, RootSessionOptions};
use session::SessionId;
use tokio::io::AsyncBufReadExt;

#[derive(Parser)]
#[command(name = "aios-dev", about = "AIOS 开发用 CLI（内核 agent loop）")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 交互式会话：空闲时打一行开新一轮；运行中打一行是 steer；/queue <文字> 排 follow-up；Ctrl-C 取消
    Chat(CommonArgs),
    /// 跑一条消息，空闲后退出；completed 退出码 0，其余非 0
    Run(RunArgs),
    /// 起 Web 服务（HTTP POST /api + WebSocket /ws，只绑本机，带启动令牌）
    Serve(ServeArgs),
    /// 配置 Keyring（key 只从标准输入读；命令行给 key 会被 clap 拒绝）
    Keyring(KeyringArgs),
}

#[derive(Args)]
struct KeyringArgs {
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// 钥匙文件位置（默认在数据目录之外的用户配置目录）
    #[arg(long)]
    identity: Option<PathBuf>,
    #[command(subcommand)]
    action: KeyringAction,
}

#[derive(Subcommand)]
enum KeyringAction {
    /// 存 key：从标准输入读，不接 --key 参数
    Set {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        /// 预设（deepseek/minimax/minimax-cn）；给了就用它的地址
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        model: String,
        #[arg(long)]
        endpoint: Option<String>,
    },
    /// 列出连接（不含 key）
    List,
    /// 删除连接
    Delete { id: String },
    /// 换钥匙文件位置（旧位置不再留）
    Relocate { to: PathBuf },
    /// 看签发记录
    Issues,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    common: CommonArgs,
    /// 绑定地址（只允许本机回环；默认 127.0.0.1）
    #[arg(long, default_value = web::DEFAULT_HOST)]
    host: String,
    /// 端口（默认 0 = 系统分配）
    #[arg(long, default_value_t = web::DEFAULT_PORT)]
    port: u16,
    /// 前端构建产物目录（默认找 ./web/dist；也可用 AIOS_WEB_DIST）
    #[arg(long)]
    web_dist: Option<PathBuf>,
    /// 仅开发/端到端测试用：覆盖 Exa 的根地址（默认 https://api.exa.ai）
    #[arg(long)]
    exa_base_url: Option<String>,
}

#[derive(Args, Clone)]
struct CommonArgs {
    /// 续接已有会话 id
    #[arg(long)]
    resume: Option<String>,
    /// 数据目录（默认 ~/.aios-dev）
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Keyring 钥匙文件位置（默认在数据目录之外的用户配置目录）
    #[arg(long)]
    identity: Option<PathBuf>,
    /// 会话工作目录（默认当前目录）
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// 连接 id（Keyring 里的连接；默认会话选择/上次选择/deepseek）
    #[arg(long)]
    connection: Option<String>,
    /// 模型 id（默认会话选择/上次选择/连接的模型）
    #[arg(long)]
    model: Option<String>,
    /// 思考档（模型自己的字符串，如 none/low/high/max；默认会话选择/上次选择/high）
    #[arg(long)]
    effort: Option<String>,
    /// 仅开发/DoD 用：覆盖模型 context window（默认 1_000_000）
    #[arg(long)]
    context_window: Option<u64>,
    /// 仅开发/DoD 用：覆盖输出 token 上限（默认模型默认值）
    #[arg(long)]
    max_tokens: Option<u64>,
    /// 仅开发/DoD 用：覆盖自动压缩阈值比例（默认 0.8）
    #[arg(long)]
    compaction_threshold: Option<f64>,
    /// 仅开发/DoD 用：覆盖压缩保留比例（默认 0.16，必须小于阈值）
    #[arg(long)]
    compaction_retain: Option<f64>,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    common: CommonArgs,
    /// 一次要跑的用户消息
    prompt: String,
}

/// CLI 错误。
#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Session(#[from] session::SessionError),
    #[error(transparent)]
    Model(#[from] model::ModelError),
    #[error(transparent)]
    Agent(#[from] agent::AgentError),
    #[error(transparent)]
    Credentials(#[from] credentials::KeyringError),
    #[error(transparent)]
    Root(#[from] service::RootError),
    #[error(transparent)]
    Web(#[from] web::ServeError),
    /// T006.18：另一个写者（服务/进程）持有会话写锁（DSH 错误码 session/writer-held）。
    #[error("会话 {id} 已被其他写者占用（session/writer-held）")]
    WriterHeld { id: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Chat(args) => chat(args).await.map(|()| 0_u8),
        Command::Run(args) => run(args).await,
        Command::Serve(args) => serve(args).await,
        Command::Keyring(args) => keyring(args),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        // T006.18：会话被别的写者占用是独立的退出码（2），便于脚本区分「忙」和一般失败。
        Err(error @ CliError::WriterHeld { .. }) => {
            eprintln!("aios-dev: {error}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("aios-dev: {error}");
            ExitCode::FAILURE
        }
    }
}

/// 仅开发/DoD 用：把 dev 覆盖参数映射到 DeepSeekConfig/AgentConfig（纯函数便于单测）。
fn config_from_args(
    common: &CommonArgs,
    base_url: String,
) -> Result<(DeepSeekConfig, AgentConfig), CliError> {
    let effort = match common.effort.as_deref() {
        None => Effort::new("high"),
        Some(raw) => Effort::parse(raw).ok_or_else(|| {
            CliError::Message(format!(
                "非法推理强度 {raw}（只接受非空字符串；模型档位规则在发送时适配）"
            ))
        })?,
    };
    let max_tokens = match common.max_tokens {
        None => DEFAULT_MAX_TOKENS,
        Some(raw) => u32::try_from(raw)
            .map_err(|_| CliError::Message(format!("--max-tokens {raw} 超出 u32 范围")))?,
    };
    if max_tokens == 0 {
        return Err(CliError::Message("--max-tokens 必须大于 0".to_owned()));
    }
    let context_window = match common.context_window {
        None => model::DEFAULT_CONTEXT_WINDOW,
        Some(0) => return Err(CliError::Message("--context-window 必须大于 0".to_owned())),
        Some(raw) => raw,
    };
    let mut agent = AgentConfig::default();
    if let Some(threshold) = common.compaction_threshold {
        agent.compaction_threshold_ratio = threshold;
    }
    if let Some(retain) = common.compaction_retain {
        agent.compaction_retain_ratio = retain;
    }
    if !agent.compaction_threshold_ratio.is_finite()
        || agent.compaction_threshold_ratio <= 0.0
        || agent.compaction_threshold_ratio > 1.0
    {
        return Err(CliError::Message(
            "--compaction-threshold 必须在 (0, 1] 之间".to_owned(),
        ));
    }
    if !agent.compaction_retain_ratio.is_finite()
        || agent.compaction_retain_ratio < 0.0
        || agent.compaction_retain_ratio >= agent.compaction_threshold_ratio
    {
        return Err(CliError::Message(
            "--compaction-retain 必须 >= 0 且小于阈值".to_owned(),
        ));
    }
    let config = DeepSeekConfig {
        provider: "deepseek".to_owned(),
        base_url,
        model: common
            .model
            .clone()
            .unwrap_or_else(|| "deepseek-flash".to_owned()),
        max_tokens,
        context_window,
        effort,
        tool_update: None,
    };
    Ok((config, agent))
}

/// T008.2：CLI 的凭证来源——每次请求向 Keyring 申请一次签发。
struct KeyringCredentialSource {
    keyring: Arc<credentials::Keyring>,
    connection: String,
    /// K4：外部 keyring set 改 key 后，签发时把监控脱敏列表刷新。
    monitor: Option<Monitor>,
    /// 屏蔽表在 settings.json（数据目录下）；聊天、重试、摘要都先过它。
    data_dir: PathBuf,
}

impl model::CredentialSource for KeyringCredentialSource {
    /// 出站前查一次屏蔽表：被拒就不签发、不发请求（和 service 的验证走同一份规则）。
    fn admit(&self, model: &str) -> Result<(), model::ModelError> {
        service::disabled::admit(&self.data_dir, &self.connection, model)
    }

    fn issue(
        &self,
        purpose: &str,
        context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        let issued = self
            .keyring
            .issue(
                &self.connection,
                purpose,
                context.run.as_deref(),
                context.turn,
            )
            .map_err(|error| {
                let message = match &error {
                    credentials::KeyringError::KeyFileMissing(_)
                    | credentials::KeyringError::Crypto(_) => {
                        format!("Keyring 无法签发：{error}")
                    }
                    _ => format!("凭证不可用：{error}"),
                };
                let code = match &error {
                    credentials::KeyringError::KeyFileMissing(_)
                    | credentials::KeyringError::Crypto(_) => model::ErrorCode::Keyring,
                    _ => model::ErrorCode::MissingCredential,
                };
                model::ModelError::new(code, message)
            })?;
        if let Some(monitor) = &self.monitor {
            refresh_monitor_secrets(monitor, &self.keyring);
        }
        Ok(issued.into_secret())
    }

    /// T012.6：连接上记住的鉴权头（None = x-api-key 与 Authorization 都发）。
    fn auth_header(&self) -> Option<String> {
        self.keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == self.connection)
            .and_then(|connection| connection.auth_header)
    }

    /// T012.6：适配器学到「只发某一个头」后写回连接（ConnectionInfo.auth_header）。
    fn remember_auth_header(&self, header: &str) {
        let Some(connection) = self
            .keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == self.connection)
        else {
            return;
        };
        // 只改 auth_header；endpoint/model 原样传回，其余字段保持不动。
        #[expect(
            clippy::let_underscore_must_use,
            reason = "记住鉴权头是尽力优化；写回失败下次请求重新学"
        )]
        let _ = self.keyring.update_connection_details(
            &self.connection,
            credentials::ConnectionUpdate {
                endpoint: connection.endpoint.clone(),
                model: connection.primary_model().to_owned(),
                auth_header: Some(header.to_owned()),
                ..credentials::ConnectionUpdate::default()
            },
        );
    }
}

/// T012.7 审计 P2：解析目标 (连接, 模型, 档)。
///
/// 规则：命令行 > 会话日志选择（resume 恢复）> settings 上次选择 > 连接默认；
/// **换了连接就整组取连接默认**——不把旧连接的模型/档带过去，避免 endpoint × 模型错配。
fn resolve_selection(
    common: &CommonArgs,
    last: Option<&session::ModelSelection>,
    connections: &[credentials::ConnectionInfo],
    data_dir: &std::path::Path,
) -> Option<(credentials::ConnectionInfo, String, String)> {
    let preferred = common
        .connection
        .clone()
        .or_else(|| last.map(|selection| selection.connection.clone()));
    let connection = preferred
        .as_ref()
        .and_then(|id| {
            connections
                .iter()
                .find(|connection| &connection.id == id)
                .cloned()
        })
        .or_else(|| {
            connections
                .iter()
                .find(|connection| connection.id == "deepseek")
                .cloned()
        })?;
    let same_connection = last.is_some_and(|selection| selection.connection == connection.id);
    let last = same_connection.then_some(last).flatten();
    let model = common
        .model
        .clone()
        .or_else(|| last.map(|selection| selection.model.clone()))
        .unwrap_or_else(|| service::models::primary_model(data_dir, &connection));
    let effort = common
        .effort
        .clone()
        .or_else(|| last.map(|selection| selection.effort_wanted.clone()))
        .unwrap_or_else(|| "high".to_owned());
    Some((connection, model, effort))
}

/// 打开数据目录的 Keyring，按会话选择解析 (连接 id, 模型配置, 上次选择的 Agent 配置, 凭证来源)。
///
/// T012.7：优先级 命令行 > 会话日志的选择（重开恢复）> settings.json 上次选择 > 连接默认。
fn model_with_keyring(
    common: &CommonArgs,
    data_dir: &PathBuf,
    monitor: Option<&Monitor>,
    session_selection: Option<&session::ModelSelection>,
) -> Result<
    (
        String,
        DeepSeekConfig,
        AgentConfig,
        Arc<dyn model::CredentialSource>,
    ),
    CliError,
> {
    let keyring = Arc::new(credentials::Keyring::open(
        data_dir,
        common.identity.as_deref(),
    )?);
    let view = keyring.view();
    if let Some(problem) = view.key_file.problem.clone() {
        return Err(CliError::Message(format!("Keyring 读不出来：{problem}")));
    }
    let settings = service::settings::load(data_dir);
    let last = session_selection
        .cloned()
        .or_else(|| settings.last_selection.clone());
    let (connection, model, effort) =
        resolve_selection(common, last.as_ref(), &view.connections, data_dir).ok_or_else(|| {
            CliError::Message(
                "还没有接模型：先从页面接上，或运行 aios-dev keyring set（key 从标准输入读）"
                    .to_owned(),
            )
        })?;
    let mut resolved_common = common.clone();
    resolved_common.effort = Some(effort);
    let (mut deepseek_config, agent_config) =
        config_from_args(&resolved_common, connection.endpoint.clone())?;
    deepseek_config.model = model;
    deepseek_config.provider =
        model::catalog::provider_id_for_connection(&connection.preset, &connection.endpoint);
    // 路由能力跟着模型信息走（厂商声明优先，其次目录写死的 deepseek-flash）。
    deepseek_config.tool_update =
        service::models::tool_update_for(data_dir, &connection, &deepseek_config.model);
    let source = KeyringCredentialSource {
        keyring,
        connection: connection.id.clone(),
        monitor: monitor.cloned(),
        data_dir: data_dir.clone(),
    };
    Ok((
        connection.id,
        deepseek_config,
        agent_config,
        Arc::new(source),
    ))
}

/// 唯一组装入口（T006.15）：这里只把 CLI 参数/env 变成普通配置数据；模型、工具集、
/// 会话存储的构造由 agent::Agent::open 一处完成（chat/run 共用，后续服务同样走它）。
async fn open_agent(common: &CommonArgs) -> Result<Agent, CliError> {
    let data_dir = match &common.data_dir {
        Some(path) => path.clone(),
        None => default_data_dir()?,
    };
    let cwd = match &common.cwd {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    let resume = match &common.resume {
        Some(raw) => Some(SessionId::parse(raw)?),
        None => None,
    };
    // T012.7：续接时从会话日志恢复模型选择（重开会话还是它）。
    let session_selection = resume.as_ref().and_then(|id| {
        session::SessionStore::open_read_only(&data_dir, id)
            .ok()
            .and_then(|snapshot| session::last_model_selection(snapshot.events()))
    });
    let (_connection, deepseek_config, config, credentials) =
        model_with_keyring(common, &data_dir, None, session_selection.as_ref())?;
    let agent = Agent::open(AgentOpenOptions {
        data_dir,
        cwd,
        resume,
        model: deepseek_config,
        config,
        credentials: Some(credentials),
        append_hook_factory: None,
        // CLI 的对话不装服务独有的查询工具（Kind 的根会话才装）。
        extra_tools: Vec::new(),
        tool_policy: agent::ToolPolicy::default(),
        policy_prompt: None,
    })
    .await
    .map_err(open_error)?;
    Ok(agent)
}

/// T006.18：把「写锁被别的写者持有」映射成可辨的占用错误（DSH 错误码 session/writer-held）。
fn open_error(error: agent::AgentError) -> CliError {
    if let agent::AgentError::Store(session::SessionError::AlreadyOwned { id, .. }) = &error {
        return CliError::WriterHeld { id: id.clone() };
    }
    CliError::Agent(error)
}

/// 起 Web 服务（T006.17）：找回/新建 Kind 根会话，起只绑本机的 HTTP+WS 载体。
/// 启动令牌只在本进程内生成并打印带令牌的 URL；不写日志/证据，不读环境变量。
/// T011.4：一条 ops 记录（attributes 带 telemetry.op）。
fn record_ops(
    monitor: &Monitor,
    op: &str,
    severity: Severity,
    attributes: Vec<(&str, serde_json::Value)>,
) {
    let mut map = serde_json::Map::new();
    map.insert("telemetry.op".to_owned(), serde_json::json!(op));
    for (key, value) in attributes {
        map.insert(key.to_owned(), value);
    }
    let _ = monitor.record(Record::new(
        Channel::Ops,
        severity,
        map,
        serde_json::json!({}),
    ));
}

/// F1：Keyring 当前持有的 key 刷新给监控（脱敏用；key 只在内存，不进日志/证据）。
fn refresh_monitor_secrets(monitor: &Monitor, keyring: &credentials::Keyring) {
    if let Ok(secrets) = keyring.secrets() {
        monitor.set_secrets(secrets);
    }
}

async fn serve(args: ServeArgs) -> Result<u8, CliError> {
    let data_dir = match &args.common.data_dir {
        Some(path) => path.clone(),
        None => default_data_dir()?,
    };
    let cwd = match &args.common.cwd {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    // T011.4：本进程的监控记录（process-start 先写；文件在 <数据目录>/monitor/ 下）。
    let monitor = Monitor::open(&data_dir).map_err(|error| CliError::Message(error.to_string()))?;
    // T012.5：目录先用缓存/内置快照（不等网络），后台刷新 models.dev。
    let catalog_source = model::catalog::bootstrap_global(&data_dir);
    record_ops(
        &monitor,
        "catalog",
        Severity::Info,
        vec![(
            "source",
            serde_json::json!(match catalog_source {
                model::catalog::CatalogSource::Network => "network",
                model::catalog::CatalogSource::Cache => "cache",
                model::catalog::CatalogSource::Snapshot => "snapshot",
            }),
        )],
    );
    {
        let catalog_data_dir = data_dir.clone();
        tokio::spawn(async move {
            let _ = model::catalog::refresh_global(&catalog_data_dir, std::time::SystemTime::now())
                .await;
        });
    }
    record_ops(
        &monitor,
        "process-start",
        Severity::Info,
        vec![
            ("version", serde_json::json!(env!("CARGO_PKG_VERSION"))),
            (
                "protocolVersion",
                serde_json::json!(service::PROTOCOL_VERSION),
            ),
            ("dataDir", serde_json::json!(data_dir.display().to_string())),
        ],
    );
    // T011.5：agent 的警告（五处 eprintln 出口）同时记成监控 ops；标准错误照旧。
    {
        let monitor_for_agent = monitor.clone();
        agent::set_warning_hook(Arc::new(move |message: &str| {
            record_ops(
                &monitor_for_agent,
                "agent-warn",
                Severity::Warn,
                vec![("message", serde_json::json!(message))],
            );
        }));
    }
    // T008.F11：cookie 签名钥匙是 Keyring 里的一条记录——首次启动自动建，之后每次启动读回；
    // 删记录/重新开始后下次启动换新，旧页面会话全部失效。只在内存里用，不落日志。
    let keyring =
        match credentials::Keyring::open(data_dir.clone(), args.common.identity.as_deref()) {
            Ok(keyring) => {
                record_ops(
                    &monitor,
                    "keyring",
                    Severity::Info,
                    vec![
                        ("action", serde_json::json!("open")),
                        ("result", serde_json::json!("ok")),
                        ("code", serde_json::json!("ok")),
                    ],
                );
                keyring
            }
            Err(error) => {
                record_ops(
                    &monitor,
                    "keyring",
                    Severity::Warn,
                    vec![
                        ("action", serde_json::json!("open")),
                        ("result", serde_json::json!("failed")),
                        ("code", serde_json::json!("keyring/error")),
                    ],
                );
                return Err(error.into());
            }
        };
    refresh_monitor_secrets(&monitor, &keyring);
    let session_secret = keyring.cookie_signing_secret()?;
    // T008.3b：模型配置惰性解析——空 Keyring 也能起服务（首次使用页先显示）；
    // recheck 时工厂重新读当前 Keyring 连接，用新模型重建根会话。
    let (placeholder_config, agent_config) =
        config_from_args(&args.common, "http://127.0.0.1:1".to_owned())?;
    let window = placeholder_config.context_window;
    let factory_common = args.common.clone();
    let factory_data_dir = data_dir.clone();
    let monitor_for_recheck = monitor.clone();
    let model_factory: service::ModelFactory = std::sync::Arc::new(move |selection| {
        if let Ok(keyring) =
            credentials::Keyring::open(factory_data_dir.clone(), factory_common.identity.as_deref())
        {
            refresh_monitor_secrets(&monitor_for_recheck, &keyring);
        }
        model_with_keyring(
            &factory_common,
            &factory_data_dir,
            Some(&monitor_for_recheck),
            selection,
        )
        .map(|(connection, config, _agent, credentials)| (connection, config, Some(credentials)))
        .map_err(|error| error.to_string())
    });
    // F1：设置页保存/删除/撤销、搬迁/重开/重启后，把 Keyring 当前 key 刷新给监控。
    let monitor_for_keyring = monitor.clone();
    let keyring_changed: service::KeyringChanged =
        Arc::new(move |keyring: &credentials::Keyring| {
            refresh_monitor_secrets(&monitor_for_keyring, keyring);
        });
    // T011.4/F1：ledger 钩子挂到根会话的「追加之后」；脱敏在监控写线程出口统一做。
    let append_hook_factory = monitor.append_hook_factory();
    // T007.2：根会话打不开时服务照样起；ready 报状态（path/line），方法一律拒绝，
    // 页面对应五种说明页；修好后页面调 recheck 再检查一次。
    let identity = args.common.identity.clone();
    let actions = std::sync::Arc::new(service::ActionRegistry::settings(
        data_dir.clone(),
        identity.clone(),
    ));
    let host = RootHost::open(
        RootSessionOptions {
            data_dir,
            cwd,
            model: placeholder_config,
            config: agent_config,
            credentials: None,
            identity,
            model_factory: Some(model_factory),
            append_hook_factory: Some(append_hook_factory),
            keyring_changed: Some(keyring_changed),
            actions,
            exa_base_url: args.exa_base_url.clone(),
            web_listen: None,
        },
        window,
    )
    .await;
    let static_dir = args
        .web_dist
        .clone()
        .or_else(|| std::env::var_os("AIOS_WEB_DIST").map(PathBuf::from))
        .or_else(|| {
            let candidate = PathBuf::from("web/dist");
            candidate.is_dir().then_some(candidate)
        });
    let token = web::launch_token();
    let handle = web::serve(
        web::ServeOptions {
            host: args.host.clone(),
            port: args.port,
            token: token.clone(),
            session_secret,
            static_dir,
            monitor: Some(monitor.clone()),
        },
        host,
    )
    .await?;
    println!(
        "aios-dev serve: http://{}:{}/?token={}",
        args.host,
        handle.addr.port(),
        handle.token
    );
    tokio::signal::ctrl_c().await?;
    // T006.18：停服务收尾（取消正在跑的一轮、日志收尾完整）；骨架阶段等价 abort。
    handle.shutdown().await;
    // T011.4：退出写 shutdown 并排空监控队列。
    record_ops(&monitor, "shutdown", Severity::Info, vec![]);
    monitor.shutdown();
    Ok(0)
}

fn default_data_dir() -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CliError::Message("找不到 HOME，无法确定 ~/.aios-dev".to_owned()))?;
    Ok(PathBuf::from(home).join(".aios-dev"))
}

/// chat 一行输入如何分派（T005.11：pending question 时下一行是答案）。
#[derive(Debug, PartialEq, Eq)]
enum ChatInput {
    Answer(String),
    Exit,
    Compact,
    /// /compact 带了参数：照 DSH 只打 usage，不当成普通消息。
    CompactUsage,
    Queue(String),
    FollowUp(String),
    Steer(String),
    Ignore,
}

fn dispatch_chat_input(
    line: &str,
    running: bool,
    pending_question: Option<&agent::Question>,
) -> ChatInput {
    let line = line.trim();
    if line.is_empty() {
        return ChatInput::Ignore;
    }
    // T005.11：等待问题时，下一行输入就是答案（包括 /compact、/queue、普通文本），
    // 不再产生新消息/steer/queue，避免与后台 stdin 读者抢行。
    if pending_question.is_some() {
        return ChatInput::Answer(line.to_owned());
    }
    if line == "/exit" {
        return ChatInput::Exit;
    }
    if is_compact_command(line) {
        return ChatInput::Compact;
    }
    if let Some(rest) = line.strip_prefix("/compact")
        && !rest.trim().is_empty()
    {
        return ChatInput::CompactUsage;
    }
    if let Some(queued) = line.strip_prefix("/queue ") {
        let queued = queued.trim();
        if !queued.is_empty() {
            return ChatInput::Queue(queued.to_owned());
        }
    }
    if running {
        ChatInput::Steer(line.to_owned())
    } else {
        ChatInput::FollowUp(line.to_owned())
    }
}

/// 一行输入如何变成 DSH 答案（T006.13）：输入恰好是某个选项标签时当作选项
/// （selected=[label]）；多选问题还接受逗号分隔的多个标签；其余一律当自定义文本（custom）。
/// 这是开发 CLI 的交互适配（DSH 没有 CLI），界面走 T006.16a 的服务方法。
fn classify_answer(answer: &str, question: &agent::Question) -> (Vec<String>, Option<String>) {
    let labels: Vec<&str> = question
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    if labels.contains(&answer) {
        return (vec![answer.to_owned()], None);
    }
    if question.multi_select {
        let selected: Vec<String> = answer
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        if !selected.is_empty()
            && selected
                .iter()
                .all(|label| labels.contains(&label.as_str()))
        {
            return (selected, None);
        }
    }
    (Vec::new(), Some(answer.to_owned()))
}

/// 事件如何改变待答问题队列（T005.12）：QuestionAsked 入队、TurnEnded 清空。
fn pending_questions_after_event(
    current: std::collections::VecDeque<agent::Question>,
    event: &AgentEvent,
) -> std::collections::VecDeque<agent::Question> {
    match event {
        AgentEvent::QuestionAsked { questions } => questions.iter().cloned().collect(),
        AgentEvent::TurnEnded { .. } => std::collections::VecDeque::new(),
        _ => current,
    }
}

async fn chat(args: CommonArgs) -> Result<(), CliError> {
    let agent = open_agent(&args).await?;
    println!("session: {}", agent.id());
    println!("log: {}", agent.log_path().display());
    let mut events = agent.subscribe();
    // stdin 读取放到独立任务里，主循环用可取消的 channel 接收行；
    // 否则 select 里反复丢弃 next_line 的未来有丢半行的风险。
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut running = false;
    let mut last_outcome = None;
    let mut pending_questions: std::collections::VecDeque<agent::Question> =
        std::collections::VecDeque::new();
    loop {
        if !running {
            print!("> ");
            io::stdout().flush()?;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                // 清空收件箱并中止当前一轮；已流出的文字按 interrupted 收尾。
                agent.cancel().await?;
                pending_questions.clear();
                println!();
            }
            line = line_rx.recv() => {
                let Some(line) = line else {
                    println!();
                    break;
                };
                match dispatch_chat_input(&line, running, pending_questions.front()) {
                    ChatInput::Ignore => continue,
                    ChatInput::Answer(answer) => {
                        if let Some(question) = pending_questions.pop_front() {
                            let (selected, custom) = classify_answer(&answer, &question);
                            if agent.answer(&question.id, selected, custom).await {
                                if let Some(next) = pending_questions.front() {
                                    println!();
                                    println!("{}", question_asked_text(next));
                                }
                            } else {
                                pending_questions.push_front(question);
                            }
                        }
                    }
                    ChatInput::Exit => break,
                    ChatInput::Compact => {
                        // 空闲时立刻压缩；运行中排到当前轮之后。结果照 DSH command-compact
                        // 打一行计数（自动压缩不再往 CLI 打 start/end 行）。
                        let agent = agent.clone();
                        let command_id = new_command_id();
                        tokio::spawn(async move {
                            match agent.compact(Some(command_id)).await {
                                Ok(outcome) => println!("{}", compact_result_text(outcome)),
                                Err(error) => println!(
                                    "{}",
                                    compact_failure_text(&error)
                                        .unwrap_or_else(|| format!("Compaction failed: {error}."))
                                ),
                            }
                        });
                    }
                    ChatInput::CompactUsage => {
                        println!();
                        println!("{COMPACT_USAGE}");
                    }
                    ChatInput::Queue(queued) => {
                        agent.send(UserMessage::text(queued), Delivery::FollowUp).await?;
                        running = true;
                    }
                    ChatInput::FollowUp(text) => {
                        agent.send(UserMessage::text(text), Delivery::FollowUp).await?;
                        running = true;
                    }
                    ChatInput::Steer(text) => {
                        agent.send(UserMessage::text(text), Delivery::Steer).await?;
                    }
                }
            }
            received = events.recv() => match received {
                Ok(event) => {
                    if let AgentEvent::QuestionAsked { questions } = &event {
                        pending_questions = questions.iter().cloned().collect();
                        if let Some(first) = pending_questions.front() {
                            println!();
                            println!("{}", question_asked_text(first));
                        }
                    } else {
                        let state = std::mem::take(&mut pending_questions);
                        handle_event(event.clone(), &mut last_outcome);
                        pending_questions = pending_questions_after_event(state, &event);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            _ = agent.idle(), if running => {
                running = false;
            }
        }
    }
    // Ctrl-D / /exit：把还没收尾的一轮取消掉，等 idle 再退出。
    agent.cancel().await?;
    agent.idle().await;
    Ok(())
}

async fn run(args: RunArgs) -> Result<u8, CliError> {
    let agent = open_agent(&args.common).await?;
    println!("session: {}", agent.id());
    println!("log: {}", agent.log_path().display());
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text(args.prompt), Delivery::FollowUp)
        .await?;
    match pump_until_idle(&agent, &mut events).await {
        Some(TurnOutcome::Completed) => Ok(0),
        _ => Ok(1),
    }
}

/// 一边打印事件，一边等到 agent 空闲；返回最后一轮的结束结果。
async fn pump_until_idle(
    agent: &Agent,
    events: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
) -> Option<TurnOutcome> {
    let mut outcome = None;
    let idle = agent.idle();
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = &mut idle => break,
            received = events.recv() => match received {
                Ok(event) => {
                    let asked = run_should_cancel_for_question(&event);
                    handle_event(event, &mut outcome);
                    // T005.10 非交互 run：不能静默卡死；打印问题并撤销这一轮。
                    if asked {
                        eprintln!("[非交互模式无法回答问题，已取消本轮]");
                        if let Err(error) = agent.cancel().await {
                            eprintln!("aios-dev: {error}");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    while let Ok(event) = events.try_recv() {
        handle_event(event, &mut outcome);
    }
    outcome
}

fn handle_event(event: AgentEvent, outcome: &mut Option<TurnOutcome>) {
    match event {
        AgentEvent::AssistantTextDelta { text } => {
            print!("{text}");
            #[expect(
                clippy::let_underscore_must_use,
                reason = "终端关闭时刷新失败是预期；不改变事件处理"
            )]
            let _ = io::stdout().flush();
        }
        AgentEvent::Retrying {
            attempt,
            max_attempts,
            delay_ms,
            code,
        } => {
            println!();
            println!("[正在重试 {attempt}/{max_attempts}：{code}，约 {delay_ms}ms 后]");
        }
        AgentEvent::UsageReported { usage } => {
            println!();
            println!("用量: {}", usage.summary());
        }
        AgentEvent::QuestionAsked { questions } => {
            println!();
            println!("{}", questions_asked_text(&questions));
        }
        // T006.3：压缩是日志侧/命令侧行为（DSH command-compact 只打结果行），
        // 自动压缩不往 CLI 打 start/end 行；这些事件留给服务/界面层消费。
        AgentEvent::CompactionStarted { .. } | AgentEvent::CompactionEnded { .. } => {}
        AgentEvent::JobCompleted {
            job_id,
            status,
            detail,
        } => {
            println!();
            println!("{}", job_completed_text(&job_id, status, detail.as_deref()));
        }
        // T006.2a：输入队列通知只给界面/服务用；开发 CLI 不显示。
        // T015.5：工具参数增量同样只给页面（「正在画 · 标题」），CLI 不显示。
        AgentEvent::InboxInserted { .. }
        | AgentEvent::InboxClaimed { .. }
        | AgentEvent::InboxDiscarded { .. }
        | AgentEvent::ToolCallDelta { .. } => {}
        AgentEvent::TurnEnded { outcome: ended, .. } => {
            println!();
            match &ended {
                TurnOutcome::Completed => {}
                TurnOutcome::MaxTokens => println!("[结束原因: max-tokens]"),
                TurnOutcome::Aborted => println!("[已取消]"),
                TurnOutcome::Error { code, message } => eprintln!("[错误 {code}] {message}"),
            }
            *outcome = Some(ended);
        }
    }
}

/// 非交互 run 收到问题时的决策：打印提示并取消本轮（不静默卡死；测试清单第 14 条）。
fn run_should_cancel_for_question(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::QuestionAsked { .. })
}

/// 问题展示文本（纯函数便于单测；测试清单第 14 条）。
/// 一次事件里全部问题的展示文本（run 非交互路径用；chat 逐题打印）。
fn questions_asked_text(questions: &[agent::Question]) -> String {
    questions
        .iter()
        .map(question_asked_text)
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn question_asked_text(question: &agent::Question) -> String {
    let mut text = format!("[问题：{}]", question.question);
    if let Some(header) = &question.header {
        text = format!("[问题：{header} / {}]", question.question);
    }
    for (index, option) in question.options.iter().enumerate() {
        match &option.description {
            Some(description) => {
                text.push_str(&format!(
                    "\n  {}. {}（{}）",
                    index + 1,
                    option.label,
                    description
                ));
            }
            None => text.push_str(&format!("\n  {}. {}", index + 1, option.label)),
        }
    }
    if question.multi_select {
        text.push_str("\n  （可多选）");
    }
    text
}

/// chat 的手动压缩命令（T005.8b）：/compact。
fn is_compact_command(line: &str) -> bool {
    line.trim() == "/compact"
}

/// /compact 的用法提示（照 DSH command-compact 的 USAGE）。
const COMPACT_USAGE: &str = "Usage: /compact (no arguments)";

/// /compact 失败的逐码文案（T006.19 照 DSH command-compact 的 expectedFailure：
/// 六类 ManualCompactionErrorCode 各一句稳定文案；非手动压缩错误不接管）。
fn compact_failure_text(error: &agent::AgentError) -> Option<String> {
    let agent::AgentError::ManualCompaction { code, .. } = error else {
        return None;
    };
    Some(code.text().to_owned())
}

/// /compact 的结果文本（照 DSH command-compact：成功带替换节点数与估算 token，
/// 没有可压缩历史时给一句明确的话）。
fn compact_result_text(outcome: Option<CompactionOutcome>) -> String {
    match outcome {
        Some(outcome) => format!(
            "Compacted {} history items (~{} tokens).",
            outcome.shadowed_items, outcome.shadowed_tokens
        ),
        None => "No compactable history yet.".to_owned(),
    }
}

/// 为一次手动 /compact 生成发起命令 id（写进 compaction/* 的 sourceCommandId）。
fn new_command_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    format!("command-{}-{nanos}", std::process::id())
}

/// T005.4 票面：后台任务完成事件在 CLI 的展示文本（纯函数便于单测）。
fn job_completed_text(job_id: &str, status: JobStatus, detail: Option<&str>) -> String {
    let status = match status {
        JobStatus::Completed => "completed",
        JobStatus::Failed => "failed",
        JobStatus::Killed => "killed",
    };
    match detail {
        Some(detail) => format!("[任务完成：{job_id}（{status}）：{detail}]"),
        None => format!("[任务完成：{job_id}（{status}）]"),
    }
}

#[cfg(test)]
mod tests {
    use agent::{AgentConfig, AgentEvent, CompactionOutcome, JobStatus};
    use model::DEFAULT_MAX_TOKENS;

    use super::{
        ChatInput, CommonArgs, classify_answer, compact_result_text, config_from_args,
        dispatch_chat_input, is_compact_command, job_completed_text, pending_questions_after_event,
        question_asked_text, questions_asked_text, run_should_cancel_for_question,
    };

    #[test]
    fn job_completed_line_is_rendered() {
        assert_eq!(
            job_completed_text("bash-1", JobStatus::Completed, Some("echo hi")),
            "[任务完成：bash-1（completed）：echo hi]"
        );
        assert_eq!(
            job_completed_text("bash-2", JobStatus::Killed, None),
            "[任务完成：bash-2（killed）]"
        );
        assert_eq!(
            job_completed_text("bash-3", JobStatus::Failed, None),
            "[任务完成：bash-3（failed）]"
        );
    }

    #[test]
    fn compact_command_is_recognized() {
        assert!(is_compact_command("/compact"));
        assert!(is_compact_command("  /compact  "));
        assert!(!is_compact_command("/compact now"));
        assert!(!is_compact_command("compact"));
    }

    #[test]
    fn compact_result_lines_follow_dsh_command() {
        // DSH command-compact：成功打替换节点数与估算 token；没有历史给明确一句。
        assert_eq!(
            compact_result_text(Some(CompactionOutcome {
                shadowed_items: 3,
                shadowed_tokens: 1200,
            })),
            "Compacted 3 history items (~1200 tokens)."
        );
        assert_eq!(compact_result_text(None), "No compactable history yet.");
    }
    #[test]
    fn question_text_renders_header_and_options() {
        let question = agent::Question {
            id: "q1".to_owned(),
            question: "Proceed?".to_owned(),
            header: Some("Confirm".to_owned()),
            options: vec![
                agent::QuestionOption {
                    label: "Yes".to_owned(),
                    description: Some("go".to_owned()),
                },
                agent::QuestionOption {
                    label: "No".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        };
        let text = question_asked_text(&question);
        assert!(text.contains("Confirm"));
        assert!(text.contains("Proceed?"));
        assert!(text.contains("1. Yes（go）"));
        assert!(text.contains("2. No"));
    }

    #[test]
    fn multi_question_chat_flow_answers_sequentially() {
        let question = |id: &str| agent::Question {
            id: id.to_owned(),
            question: format!("Q {id}"),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        let mut queue: std::collections::VecDeque<agent::Question> =
            vec![question("q1"), question("q2")].into();
        assert_eq!(
            dispatch_chat_input("A1", true, queue.front()),
            ChatInput::Answer("A1".to_owned())
        );
        queue.pop_front();
        assert_eq!(queue.front().map(|q| q.id.as_str()), Some("q2"));
        assert_eq!(
            dispatch_chat_input("A2", true, queue.front()),
            ChatInput::Answer("A2".to_owned())
        );
    }

    #[test]
    fn questions_asked_text_renders_all_questions() {
        let question = |id: &str, text: &str| agent::Question {
            id: id.to_owned(),
            question: text.to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        let text = questions_asked_text(&[question("q1", "First?"), question("q2", "Second?")]);
        assert!(text.contains("First?"));
        assert!(text.contains("Second?"));
    }

    #[test]
    fn dispatch_chat_input_routes_pending_question_to_answer() {
        let question = agent::Question {
            id: "q1".to_owned(),
            question: "Proceed?".to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        for line in ["选 B", "/queue 普通", "/compact", "普通消息"] {
            assert_eq!(
                dispatch_chat_input(line, true, Some(&question)),
                ChatInput::Answer(line.trim().to_owned()),
                "pending question 时任何输入都作为答案: {line}"
            );
        }
    }

    #[test]
    fn dispatch_chat_input_without_question_is_unchanged() {
        assert_eq!(dispatch_chat_input("  ", true, None), ChatInput::Ignore);
        assert_eq!(dispatch_chat_input("/exit", true, None), ChatInput::Exit);
        assert_eq!(
            dispatch_chat_input("  /compact ", true, None),
            ChatInput::Compact
        );
        assert_eq!(
            dispatch_chat_input("/compact now", true, None),
            ChatInput::CompactUsage,
            "/compact 带参数只打 usage（DSH 行为），不当普通消息"
        );
        assert_eq!(
            dispatch_chat_input("/queue 做别的", true, None),
            ChatInput::Queue("做别的".to_owned())
        );
        assert_eq!(
            dispatch_chat_input("继续", true, None),
            ChatInput::Steer("继续".to_owned())
        );
        assert_eq!(
            dispatch_chat_input("新任务", false, None),
            ChatInput::FollowUp("新任务".to_owned())
        );
    }

    #[test]
    fn dispatch_chat_input_resumes_normal_after_question() {
        let question = agent::Question {
            id: "q1".to_owned(),
            question: "Proceed?".to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        assert_eq!(
            dispatch_chat_input("选 B", true, Some(&question)),
            ChatInput::Answer("选 B".to_owned())
        );
        assert_eq!(
            dispatch_chat_input("继续", true, None),
            ChatInput::Steer("继续".to_owned()),
            "问题结束后恢复普通输入"
        );
    }

    #[test]
    fn classify_answer_maps_option_labels_and_custom_text() {
        // 照 DSH 答案项（K14）：标签是选项 → selected；其余文本 → custom；多选可逗号选多个。
        let question = |multi_select: bool| agent::Question {
            id: "q1".to_owned(),
            question: "Pick".to_owned(),
            header: None,
            options: vec![
                agent::QuestionOption {
                    label: "Yes".to_owned(),
                    description: None,
                },
                agent::QuestionOption {
                    label: "No".to_owned(),
                    description: None,
                },
            ],
            multi_select,
        };
        assert_eq!(
            classify_answer("Yes", &question(false)),
            (vec!["Yes".to_owned()], None),
            "输入恰好是选项标签 → selected"
        );
        assert_eq!(
            classify_answer("我自己的答案", &question(false)),
            (Vec::new(), Some("我自己的答案".to_owned())),
            "不在选项里的输入 → custom"
        );
        assert_eq!(
            classify_answer("Yes, No", &question(true)),
            (vec!["Yes".to_owned(), "No".to_owned()], None),
            "多选问题：逗号分隔且每项都是选项 → selected 多个"
        );
        assert_eq!(
            classify_answer("Yes, maybe", &question(true)),
            (Vec::new(), Some("Yes, maybe".to_owned())),
            "有一项不是选项时整行当 custom"
        );
    }

    #[test]
    fn pending_questions_queue_holds_all_and_clears_on_turn_end() {
        let q = |id: &str| agent::Question {
            id: id.to_owned(),
            question: "Proceed?".to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        let queue = pending_questions_after_event(
            std::collections::VecDeque::new(),
            &AgentEvent::QuestionAsked {
                questions: vec![q("q1"), q("q2")],
            },
        );
        assert_eq!(queue.len(), 2, "多问题都要进队列");
        assert_eq!(
            queue.front().map(|question| question.id.as_str()),
            Some("q1")
        );
        assert_eq!(
            pending_questions_after_event(
                queue,
                &AgentEvent::TurnEnded {
                    turn: 1,
                    outcome: agent::TurnOutcome::Aborted
                }
            )
            .len(),
            0
        );
    }

    #[test]
    fn non_interactive_question_decision_cancels_turn() {
        let question = agent::Question {
            id: "q1".to_owned(),
            question: "Proceed?".to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        assert!(run_should_cancel_for_question(&AgentEvent::QuestionAsked {
            questions: vec![question]
        }));
        assert!(!run_should_cancel_for_question(
            &AgentEvent::AssistantTextDelta {
                text: "x".to_owned()
            }
        ));
    }

    fn dev_common() -> CommonArgs {
        CommonArgs {
            resume: None,
            data_dir: None,
            identity: None,
            cwd: None,
            connection: None,
            model: None,
            effort: None,
            context_window: None,
            max_tokens: None,
            compaction_threshold: None,
            compaction_retain: None,
        }
    }

    fn connection_info(id: &str, model: &str) -> credentials::ConnectionInfo {
        credentials::ConnectionInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            model: model.to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            saved_at: 0,
            check: credentials::CheckState::Never,
            key_saved: true,
            preset: String::new(),
            models: Some(vec![credentials::ModelRow {
                id: model.to_owned(),
                name: None,
                context_window: None,
                max_tokens: None,
                price_input: None,
                price_output: None,
                price_cache_read: None,
                price_cache_write: None,
            }]),
            auth_header: None,
        }
    }

    /// T012.7 审计 P2：只给 --connection B 时，模型/档取 B 的默认，不把 A 的带过去。
    #[test]
    fn switching_connection_resets_model_and_effort_to_connection_defaults() {
        let mut common = dev_common();
        common.connection = Some("minimax-cn".to_owned());
        let last = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-pro".to_owned(),
            effort_wanted: "low".to_owned(),
        };
        let connections = vec![
            connection_info("deepseek", "deepseek-flash"),
            connection_info("minimax-cn", "MiniMax-M3.1-Flash"),
        ];
        let (connection, model, effort) =
            crate::resolve_selection(&common, Some(&last), &connections, &std::env::temp_dir())
                .expect("resolved");
        assert_eq!(connection.id, "minimax-cn");
        assert_eq!(
            model, "MiniMax-M3.1-Flash",
            "换连接整组取连接默认：不带旧连接的模型"
        );
        assert_eq!(effort, "high", "换连接取默认档");
    }

    /// 同一连接保留上次选择；命令行显式覆盖最优先。
    #[test]
    fn same_connection_keeps_last_selection_and_cli_overrides_win() {
        let connections = vec![
            connection_info("deepseek", "deepseek-flash"),
            connection_info("minimax-cn", "MiniMax-M3.1-Flash"),
        ];
        let last = session::ModelSelection {
            connection: "deepseek".to_owned(),
            model: "deepseek-pro".to_owned(),
            effort_wanted: "low".to_owned(),
        };
        let mut common = dev_common();
        common.connection = Some("deepseek".to_owned());
        let (_, model, effort) =
            crate::resolve_selection(&common, Some(&last), &connections, &std::env::temp_dir())
                .expect("resolved");
        assert_eq!(model, "deepseek-pro");
        assert_eq!(effort, "low");
        common.model = Some("explicit-model".to_owned());
        common.effort = Some("max".to_owned());
        let (_, model, effort) =
            crate::resolve_selection(&common, Some(&last), &connections, &std::env::temp_dir())
                .expect("resolved");
        assert_eq!(model, "explicit-model");
        assert_eq!(effort, "max");
    }

    #[test]
    fn config_from_args_defaults_match_production_defaults() {
        let (deepseek, agent) = config_from_args(&dev_common(), "http://x".to_owned()).unwrap();
        assert_eq!(deepseek.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(deepseek.context_window, model::DEFAULT_CONTEXT_WINDOW);
        assert_eq!(
            agent.compaction_threshold_ratio,
            AgentConfig::default().compaction_threshold_ratio
        );
        assert_eq!(
            agent.compaction_retain_ratio,
            AgentConfig::default().compaction_retain_ratio
        );
    }

    #[test]
    fn config_from_args_applies_dev_overrides() {
        let common = CommonArgs {
            context_window: Some(24_000),
            max_tokens: Some(1024),
            compaction_threshold: Some(0.2),
            compaction_retain: Some(0.1),
            ..dev_common()
        };
        let (deepseek, agent) = config_from_args(&common, "http://x".to_owned()).unwrap();
        assert_eq!(deepseek.context_window, 24_000);
        assert_eq!(deepseek.max_tokens, 1024);
        assert_eq!(agent.compaction_threshold_ratio, 0.2);
        assert_eq!(agent.compaction_retain_ratio, 0.1);
    }

    #[test]
    fn config_from_args_rejects_invalid_values() {
        let cases = vec![
            CommonArgs {
                max_tokens: Some(0),
                ..dev_common()
            },
            CommonArgs {
                max_tokens: Some(u64::from(u32::MAX) + 1),
                ..dev_common()
            },
            CommonArgs {
                context_window: Some(0),
                ..dev_common()
            },
            CommonArgs {
                compaction_threshold: Some(0.0),
                ..dev_common()
            },
            CommonArgs {
                compaction_threshold: Some(1.5),
                ..dev_common()
            },
            CommonArgs {
                compaction_retain: Some(-0.1),
                ..dev_common()
            },
            CommonArgs {
                compaction_threshold: Some(0.2),
                compaction_retain: Some(0.2),
                ..dev_common()
            },
        ];
        for common in cases {
            assert!(
                config_from_args(&common, "http://x".to_owned()).is_err(),
                "非法值必须报错"
            );
        }
    }
}

/// T006.19 红测试：/compact 失败文案逐码照 DSH command-compact 的 expectedFailure。
#[cfg(test)]
mod t006_19_compact_failure_tests {
    use agent::{AgentError, ManualCompactionErrorCode};

    use crate::compact_failure_text;

    #[test]
    fn compact_failure_texts_follow_dsh_command() {
        let cases = [
            (
                ManualCompactionErrorCode::Busy,
                "Compaction is unavailable because this process has an active compaction, or the agent is not idle.",
            ),
            (
                ManualCompactionErrorCode::Cancelled,
                "Compaction cancelled.",
            ),
            (
                ManualCompactionErrorCode::Changed,
                "The history selected for compaction changed before it could be replaced. The attempt is recorded in the session log.",
            ),
            (
                ManualCompactionErrorCode::Summary,
                "Compaction could not produce a useful summary. The attempt is recorded in the session log.",
            ),
            (
                ManualCompactionErrorCode::Commit,
                "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying.",
            ),
            (
                ManualCompactionErrorCode::Persistence,
                "Compaction finished, but the session could not be saved.",
            ),
        ];
        for (code, text) in cases {
            let error = AgentError::ManualCompaction {
                code,
                message: "backend detail".to_owned(),
            };
            assert_eq!(
                compact_failure_text(&error).as_deref(),
                Some(text),
                "code={code:?}"
            );
        }
        assert_eq!(
            compact_failure_text(&AgentError::Compaction("boom".to_owned())),
            None,
            "非手动压缩错误不给逐码文案"
        );
    }
}

fn keyring(args: KeyringArgs) -> Result<u8, CliError> {
    let data_dir = args.data_dir.unwrap_or(default_data_dir()?);
    let keyring = credentials::Keyring::open(data_dir, args.identity.as_deref())?;
    match args.action {
        KeyringAction::Set {
            id,
            name,
            preset,
            model,
            endpoint,
        } => {
            let preset = preset
                .as_deref()
                .map(|id| {
                    credentials::preset(id).ok_or_else(|| {
                        CliError::Message(format!(
                            "未知预设 {id}（可用：deepseek/minimax/minimax-cn）"
                        ))
                    })
                })
                .transpose()?;
            let endpoint = endpoint
                .or_else(|| preset.map(|preset| preset.endpoint.to_owned()))
                .ok_or_else(|| CliError::Message("--endpoint 或 --preset 必须给一个".to_owned()))?;
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            let api_key = input.trim().to_owned();
            if api_key.is_empty() {
                return Err(CliError::Message(
                    "没有从标准输入读到 key（不接 --key 参数，避免进 shell 历史）".to_owned(),
                ));
            }
            let info = keyring.save_connection(credentials::NewConnection {
                id,
                name,
                model,
                endpoint,
                api_key,
                preset: preset.map(|preset| preset.id.to_owned()),
                ..Default::default()
            })?;
            println!("已保存 {}（{}）", info.name, info.id);
            Ok(0)
        }
        KeyringAction::List => {
            let view = keyring.view();
            if let Some(problem) = &view.key_file.problem {
                println!("Keyring 读不出来：{problem}");
                println!("钥匙文件：{}", view.key_file.path.display());
                return Ok(1);
            }
            if view.connections.is_empty() {
                println!("还没有连接");
                return Ok(0);
            }
            for connection in &view.connections {
                let check = match &connection.check {
                    credentials::CheckState::Never => "还没验证".to_owned(),
                    credentials::CheckState::Ok { .. } => "连接可用".to_owned(),
                    credentials::CheckState::Fail { reason, .. } => format!("验证失败：{reason}"),
                };
                println!(
                    "{}（{}） · {} · {} · {check}",
                    connection.name, connection.id, connection.model, connection.endpoint
                );
            }
            Ok(0)
        }
        KeyringAction::Delete { id } => {
            let deleted = keyring.delete_connection(&id)?;
            if deleted {
                println!("已删除 {id}");
            } else {
                println!("没有这个连接：{id}");
            }
            Ok(0)
        }
        KeyringAction::Relocate { to } => {
            keyring.relocate_identity(&to)?;
            println!("钥匙文件已换到 {}", to.display());
            Ok(0)
        }
        KeyringAction::Issues => {
            let issues = keyring.issues();
            if issues.is_empty() {
                println!("还没有签发记录");
                return Ok(0);
            }
            for issue in issues {
                let run = issue.run.as_deref().unwrap_or("-");
                println!(
                    "#{} · {} · {} · {run}",
                    issue.id, issue.connection, issue.purpose
                );
            }
            Ok(0)
        }
    }
}

#[cfg(test)]
mod f1_redaction_tests {
    use super::*;
    use serde_json::json;
    use session::EventKind;

    fn tool_result_with_key(text: &str) -> EventKind {
        EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: model::Message::tool_result("call_1", text, false),
            error: None,
            meta: None,
        }
    }

    fn connection(id: &str, name: &str, key: &str) -> credentials::NewConnection {
        credentials::NewConnection {
            id: id.to_owned(),
            name: name.to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            api_key: key.to_owned(),
            ..Default::default()
        }
    }

    /// F1：生产装配函数（refresh_monitor_secrets + append_hook_factory）下，
    /// Keyring 启动 key、启动后保存的 key、ops 记录里的 key 都不能以原文进监控。
    #[test]
    fn production_assembly_redacts_startup_and_late_keys_everywhere() {
        let data = tempfile::tempdir().expect("数据目录");
        let cwd = tempfile::tempdir().expect("工作目录");
        let identity = data.path().join("identity.txt");
        let keyring = credentials::Keyring::open(data.path(), Some(&identity)).expect("keyring");
        keyring
            .save_connection(connection("deepseek", "DeepSeek", "sk-STARTUP-KEY-1"))
            .expect("save startup");
        let monitor = Monitor::open(data.path()).expect("monitor");
        let path = monitor.path().to_path_buf();
        refresh_monitor_secrets(&monitor, &keyring);

        let mut store = session::SessionStore::create(data.path(), cwd.path()).expect("session");
        let factory = monitor.append_hook_factory();
        store.set_append_hook(Some(factory(store.id())));
        store
            .append(&[tool_result_with_key("工具输出 sk-STARTUP-KEY-1 结束")])
            .expect("append startup");

        let mut attributes = serde_json::Map::new();
        attributes.insert("telemetry.op".to_owned(), json!("api-call"));
        attributes.insert("echo".to_owned(), json!("sk-STARTUP-KEY-1"));
        assert!(monitor.record(monitor::Record::new(
            monitor::Channel::Ops,
            monitor::Severity::Info,
            attributes,
            json!({"nested": "sk-STARTUP-KEY-1"}),
        )));

        // 启动后才保存的 key：设置页保存后走同一个刷新函数。
        keyring
            .save_connection(connection("late", "Late", "sk-LATE-KEY-2"))
            .expect("save late");
        refresh_monitor_secrets(&monitor, &keyring);
        store
            .append(&[tool_result_with_key("又输出 sk-LATE-KEY-2 结束")])
            .expect("append late");
        let mut attributes = serde_json::Map::new();
        attributes.insert("telemetry.op".to_owned(), json!("api-call"));
        attributes.insert("echo".to_owned(), json!("sk-LATE-KEY-2"));
        assert!(monitor.record(monitor::Record::new(
            monitor::Channel::Ops,
            monitor::Severity::Info,
            attributes,
            json!({"nested": "sk-LATE-KEY-2"}),
        )));

        drop(store);
        monitor.shutdown();
        let text = std::fs::read_to_string(&path).expect("monitor file");
        assert!(
            !text.contains("sk-STARTUP-KEY-1"),
            "启动 key 原文进监控：{text}"
        );
        assert!(
            !text.contains("sk-LATE-KEY-2"),
            "后存 key 原文进监控：{text}"
        );
        assert!(text.contains("[已脱敏]"), "有脱敏标记：{text}");
        assert!(
            !text.contains("redact-failed"),
            "正常记录不该 redact-failed：{text}"
        );
    }

    /// T012.6：CLI 的凭证来源把学到的鉴权头写回连接，并优先读连接上的值。
    #[test]
    fn learned_auth_header_is_written_back_to_the_connection() {
        use model::CredentialSource as _;
        let data = tempfile::tempdir().expect("数据目录");
        let identity = data.path().join("identity.txt");
        let keyring = std::sync::Arc::new(
            credentials::Keyring::open(data.path(), Some(&identity)).expect("keyring"),
        );
        keyring
            .save_connection(connection("deepseek", "DeepSeek", "sk-T0126-KEY"))
            .expect("save");
        let source = KeyringCredentialSource {
            keyring: std::sync::Arc::clone(&keyring),
            connection: "deepseek".to_owned(),
            monitor: None,
            data_dir: data.path().to_path_buf(),
        };
        assert_eq!(source.auth_header(), None, "还没学到的连接默认两个头都发");
        source.remember_auth_header("authorization");
        assert_eq!(
            source.auth_header().as_deref(),
            Some("authorization"),
            "写回后从连接读回学到的单头"
        );
        let stored = keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == "deepseek")
            .expect("connection");
        assert_eq!(stored.auth_header.as_deref(), Some("authorization"));
        assert_eq!(stored.endpoint, "http://127.0.0.1:1", "写回不动 endpoint");
        assert_eq!(stored.model, "deepseek-flash", "写回不动 model");
        assert!(
            stored
                .models
                .as_deref()
                .is_some_and(|models| models.len() == 1),
            "写回不动 models 行"
        );
    }
}
