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

/// chat 一行输入如何分派（T005.11：pending question 时下一行是答案）。
#[derive(Debug, PartialEq, Eq)]
enum ChatInput {
    Answer(String),
    Exit,
    Compact,
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
                            if agent.answer(&question.id, &answer).await {
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
                        // 空闲时立刻压缩；运行中排到当前轮之后（事件照常打印）。
                        let agent = agent.clone();
                        tokio::spawn(async move { let _ = agent.compact(None).await; });
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
        // T006.2a：输入队列通知只给界面/服务用；开发 CLI 不显示。
        AgentEvent::InboxInserted { .. }
        | AgentEvent::InboxClaimed { .. }
        | AgentEvent::InboxDiscarded { .. } => {}
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
        ChatInput, CommonArgs, compaction_ended_text, compaction_started_text, config_from_args,
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
