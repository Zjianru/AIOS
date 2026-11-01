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
    let effort = match common.effort.as_deref() {
        None => Effort::High,
        Some(raw) => Effort::parse(raw).ok_or_else(|| {
            CliError::Message(format!("非法推理强度 {raw}（只接受 off、low、high、max）"))
        })?,
    };
    let config = DeepSeekConfig {
        base_url,
        api_key,
        model: common
            .model
            .clone()
            .unwrap_or_else(|| "deepseek-flash".to_owned()),
        max_tokens: DEFAULT_MAX_TOKENS,
        context_window: model::DEFAULT_CONTEXT_WINDOW,
        effort,
    };
    let model = DeepSeek::new(config)?;
    let tools = ToolSet::builtin();
    let config = AgentConfig::default();
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
                Ok(event) => handle_event(event, &mut last_outcome),
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
                Ok(event) => handle_event(event, &mut outcome),
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
            println!("[问题：{}]", question.question);
            for (index, option) in question.options.iter().enumerate() {
                match &option.description {
                    Some(description) => {
                        println!("  {}. {}（{}）", index + 1, option.label, description);
                    }
                    None => println!("  {}. {}", index + 1, option.label),
                }
            }
            if question.multi_select {
                println!("  （可多选）");
            }
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
    use agent::{CompactionTrigger, JobStatus};

    use super::{compaction_ended_text, compaction_started_text, job_completed_text};

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
}
