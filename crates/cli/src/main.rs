//! 开发用 CLI：aios-dev chat / aios-dev run。

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use agent::{
    Agent, AgentConfig, AgentEvent, CompactionTrigger, Delivery, Deps, JobStatus, TurnOutcome,
};
use clap::{Args, Parser, Subcommand};
use model::{DEFAULT_BASE_URL, DEFAULT_MAX_TOKENS, DeepSeek, DeepSeekConfig, Effort, UserMessage};
use session::{SessionId, SessionStore};
use tokio::io::AsyncBufReadExt;
use tools::ToolSet;

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
}

#[derive(Args)]
struct CommonArgs {
    /// 续接已有会话 id
    #[arg(long)]
    resume: Option<String>,
    /// 数据目录（默认 ~/.aios-dev）
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// 会话工作目录（默认当前目录）
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// 模型 id（默认 deepseek-flash）
    #[arg(long)]
    model: Option<String>,
    /// 推理强度：off、low、high、max（默认 high）
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
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Chat(args) => chat(args).await.map(|()| 0_u8),
        Command::Run(args) => run(args).await,
    };
    match result {
        Ok(code) => ExitCode::from(code),
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
    api_key: String,
) -> Result<(DeepSeekConfig, AgentConfig), CliError> {
    let effort = match common.effort.as_deref() {
        None => Effort::High,
        Some(raw) => Effort::parse(raw).ok_or_else(|| {
            CliError::Message(format!("非法推理强度 {raw}（只接受 off、low、high、max）"))
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
        base_url,
        api_key,
        model: common
            .model
            .clone()
            .unwrap_or_else(|| "deepseek-flash".to_owned()),
        max_tokens,
        context_window,
        effort,
    };
    Ok((config, agent))
}

async fn open_agent(common: &CommonArgs) -> Result<Agent, CliError> {
    let data_dir = match &common.data_dir {
        Some(path) => path.clone(),
        None => default_data_dir()?,
    };
    let cwd = match &common.cwd {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    let api_key = std::env::var("DEEPSEEK_API_KEY").map_err(|_| {
        CliError::Message(
            "缺少 DEEPSEEK_API_KEY 环境变量（D34：开发期从环境变量读 key）".to_owned(),
        )
    })?;
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());
    let (deepseek_config, config) = config_from_args(common, base_url, api_key)?;
    let model = DeepSeek::new(deepseek_config)?;
    let tools = ToolSet::builtin();
    match &common.resume {
        Some(raw) => {
            let id = SessionId::parse(raw)?;
            let store = SessionStore::open(&data_dir, &id)?;
            Ok(Agent::resume(
                id,
                config,
                Deps {
                    model,
                    tools,
                    store,
                },
            )
            .await?)
        }
        None => {
            let store = SessionStore::create(&data_dir, &cwd)?;
            Ok(Agent::create(
                config,
                Deps {
                    model,
                    tools,
                    store,
                },
            )
            .await?)
        }
    }
}

fn default_data_dir() -> Result<PathBuf, CliError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CliError::Message("找不到 HOME，无法确定 ~/.aios-dev".to_owned()))?;
    Ok(PathBuf::from(home).join(".aios-dev"))
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
    loop {
        if !running {
            print!("> ");
            io::stdout().flush()?;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                // 清空收件箱并中止当前一轮；已流出的文字按 interrupted 收尾。
                agent.cancel();
                println!();
            }
            line = line_rx.recv() => {
                let Some(line) = line else {
                    println!();
                    break;
                };
                let line = line.trim().to_owned();
                if line.is_empty() {
                    continue;
                }
                if line == "/exit" {
                    break;
                }
                if is_compact_command(&line) {
                    // 空闲时立刻压缩；运行中排到当前轮之后（compress 在后台跑，事件照常打印）。
                    let agent = agent.clone();
                    tokio::spawn(async move { agent.compact().await; });
                    continue;
                }
                if let Some(queued) = line.strip_prefix("/queue ") {
                    let queued = queued.trim();
                    if queued.is_empty() {
                        continue;
                    }
                    agent.send(UserMessage::text(queued), Delivery::FollowUp);
                    running = true;
                } else if running {
                    agent.send(UserMessage::text(line), Delivery::Steer);
                } else {
                    agent.send(UserMessage::text(line), Delivery::FollowUp);
                    running = true;
                }
            }
            received = events.recv() => match received {
                Ok(event) => {
                    let question = match &event {
                        AgentEvent::QuestionAsked { question } => Some(question.clone()),
                        _ => None,
                    };
                    handle_event(event, &mut last_outcome);
                    // T005.10 交互路径：打出问题、读一行作答，loop 接着跑。
                    if let Some(question) = question && let Some(line) = read_stdin_line().await {
                        agent.answer(&question.id, line.trim()).await;
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
    agent.cancel();
    agent.idle().await;
    Ok(())
}

async fn run(args: RunArgs) -> Result<u8, CliError> {
    let agent = open_agent(&args.common).await?;
    println!("session: {}", agent.id());
    println!("log: {}", agent.log_path().display());
    let mut events = agent.subscribe();
    agent.send(UserMessage::text(args.prompt), Delivery::FollowUp);
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
                        agent.cancel();
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
        AgentEvent::QuestionAsked { question } => {
            println!();
            println!("{}", question_asked_text(&question));
        }
        AgentEvent::CompactionStarted {
            compaction_id,
            trigger,
        } => {
            println!();
            println!("{}", compaction_started_text(&compaction_id, trigger));
        }
        AgentEvent::CompactionEnded {
            compaction_id,
            error,
        } => println!(
            "{}",
            compaction_ended_text(&compaction_id, error.as_deref())
        ),
        AgentEvent::JobCompleted {
            job_id,
            status,
            detail,
        } => {
            println!();
            println!("{}", job_completed_text(&job_id, status, detail.as_deref()));
        }
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

/// 读一行 stdin（问题回答用；不阻塞异步 worker）。
async fn read_stdin_line() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        Some(line)
    })
    .await
    .ok()
    .flatten()
}

/// 非交互 run 收到问题时的决策：打印提示并取消本轮（不静默卡死；测试清单第 14 条）。
fn run_should_cancel_for_question(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::QuestionAsked { .. })
}

/// 问题展示文本（纯函数便于单测；测试清单第 14 条）。
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

/// T005.8a 票面：压缩开始/结束事件在 CLI 的展示文本（纯函数便于单测）。
fn compaction_started_text(compaction_id: &str, trigger: CompactionTrigger) -> String {
    let kind = match trigger {
        CompactionTrigger::Auto => "自动",
        CompactionTrigger::Manual => "手动",
    };
    format!("[压缩开始：{compaction_id}（{kind}）]")
}

fn compaction_ended_text(compaction_id: &str, error: Option<&str>) -> String {
    match error {
        Some(error) => format!("[压缩失败：{compaction_id}：{error}]"),
        None => format!("[压缩完成：{compaction_id}]"),
    }
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
    use agent::{AgentConfig, AgentEvent, CompactionTrigger, JobStatus};
    use model::DEFAULT_MAX_TOKENS;

    use super::{
        CommonArgs, compaction_ended_text, compaction_started_text, config_from_args,
        is_compact_command, job_completed_text, question_asked_text,
        run_should_cancel_for_question,
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
    fn compaction_lines_are_rendered() {
        assert_eq!(
            compaction_started_text("compaction-1", CompactionTrigger::Auto),
            "[压缩开始：compaction-1（自动）]"
        );
        assert_eq!(
            compaction_started_text("compaction-2", CompactionTrigger::Manual),
            "[压缩开始：compaction-2（手动）]"
        );
        assert_eq!(
            compaction_ended_text("compaction-1", None),
            "[压缩完成：compaction-1]"
        );
        assert_eq!(
            compaction_ended_text("compaction-1", Some("boom")),
            "[压缩失败：compaction-1：boom]"
        );
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
    fn non_interactive_question_decision_cancels_turn() {
        let question = agent::Question {
            id: "q1".to_owned(),
            question: "Proceed?".to_owned(),
            header: None,
            options: Vec::new(),
            multi_select: false,
        };
        assert!(run_should_cancel_for_question(&AgentEvent::QuestionAsked {
            question
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
            cwd: None,
            model: None,
            effort: None,
            context_window: None,
            max_tokens: None,
            compaction_threshold: None,
            compaction_retain: None,
        }
    }

    #[test]
    fn config_from_args_defaults_match_production_defaults() {
        let (deepseek, agent) =
            config_from_args(&dev_common(), "http://x".to_owned(), "k".to_owned()).unwrap();
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
        let (deepseek, agent) =
            config_from_args(&common, "http://x".to_owned(), "k".to_owned()).unwrap();
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
                config_from_args(&common, "http://x".to_owned(), "k".to_owned()).is_err(),
                "非法值必须报错"
            );
        }
    }
}
