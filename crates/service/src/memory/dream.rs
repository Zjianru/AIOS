//! 做梦：触发时机、做梦见条件、受限 agent、笔记变更、报告与撤销。
//!
//! 不常驻、不另建调度器：只在服务启动、Kind 回合结束、任务报回、页面打开、用户说整理
//! 这几个时机顺手检查一次；忙就等下一个时机，错过多次自然合成一次（按上次成功计时）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolResult};

use super::MemoryRuntime;
use crate::executor::TaskRuntime;
use crate::root::ModelFactory;
use crate::session_host::ServiceHost;

/// 检查梦的时机（只影响「忙」和「绕不绕过到期条件」，不改变条件本身）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamTrigger {
    Startup,
    TurnEnded,
    TaskRelayed,
    PageOpened,
    /// 用户明说「整理一下」：不等 24 小时/事件数，许可仍要。
    UserAsked,
}

/// 一次检查后发生的事（测试/报告用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamCheck {
    /// 条件没到（关着、没许可、忙、间隔/事件数不够）。
    Skipped(String),
    /// 已经有一轮在做梦。
    Running,
    /// 跑了，结果见报告。
    Ran(DreamReport),
    /// 跑了但失败。
    Failed(String),
}

/// 一次做梦的结果（卡片/页面的报告从 dream_run 读；这里给触发侧的返回值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamReport {
    pub run_id: String,
    pub applied: usize,
    pub skipped: Vec<String>,
}

/// 做梦时读报回的任务：只取上次成功之后的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamTaskFact {
    pub task_id: String,
    pub title: String,
    pub requirement: String,
    pub facts: Vec<String>,
}

/// 做梦读报回：after_ms 之后新增/更新的报告、判定、要求改版（原话），按任务分组。
/// 只读任务库、不改任何状态；放在记忆这边，executor 只提供任务库读写面。
pub(crate) fn task_facts(
    task_runtime: &TaskRuntime,
    after_ms: i64,
) -> Result<Vec<DreamTaskFact>, String> {
    let store = task_runtime.tasks_store();
    let ids = store.task_ids().map_err(|error| error.to_string())?;
    let mut facts = Vec::new();
    for id in ids {
        let Some(task) = store.task(&id).map_err(|error| error.to_string())? else {
            continue;
        };
        // 约定到点的触发任务只在日历上，不进做梦事实。
        if task.origin.as_ref().map(|origin| origin.role) == Some(tasks::AppointmentRole::Trigger) {
            continue;
        }
        let mut lines = Vec::new();
        for report in &task.reports {
            if report.at < after_ms {
                continue;
            }
            let state = serde_json::to_value(report.state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned());
            lines.push(format!("报告（{state}）：{}", report.summary));
            if let Some(question) = &report.question {
                lines.push(format!("问题：{question}"));
            }
        }
        for verdict in &task.verdicts {
            if verdict.at < after_ms {
                continue;
            }
            let conclusion = serde_json::to_value(verdict.conclusion)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned());
            let note = verdict.note.as_deref().unwrap_or("");
            lines.push(format!("判定（{conclusion}）：{note}"));
        }
        for version in &task.versions {
            if version.at < after_ms {
                continue;
            }
            if let Some(requirement) = &version.requirement {
                lines.push(format!(
                    "要求第 {} 版改成：{}",
                    version_at_ordinal(&task, version.at),
                    requirement
                ));
            }
        }
        if !lines.is_empty() {
            facts.push(DreamTaskFact {
                task_id: task.id.as_str().to_owned(),
                title: task.title.clone(),
                requirement: task.requirement.clone(),
                facts: lines,
            });
        }
    }
    Ok(facts)
}

/// 要求版本在任务里的序号（1 起）；用来在报回原文里标明「第几版」。
fn version_at_ordinal(task: &tasks::Task, at: i64) -> usize {
    task.versions
        .iter()
        .position(|version| version.at == at)
        .map_or(0, |index| index + 1)
}

/// 程序卡的状态按 ref 从库读；做梦相关的 ref 在 service 层解释。
pub const DREAM_PERMIT_CARD_REF: &str = "dream_permit";

static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

/// 做梦用哪条模型：配置、凭证、显示名（模型选择在触发时快照）。
type DreamModel = (
    model::DeepSeekConfig,
    Option<Arc<dyn model::CredentialSource>>,
    String,
);

/// 做梦想用的运行时：库、根会话槽、模型选择、任务事实，都只借用不拥有。
pub struct DreamRuntime {
    data_dir: PathBuf,
    cwd: PathBuf,
    memory: Arc<MemoryRuntime>,
    /// 根会话槽只弱持有：做梦任务不能因为自己还活着就把根会话写锁留着。
    inner: std::sync::Weak<tokio::sync::Mutex<Option<ServiceHost>>>,
    selection: Arc<Mutex<Option<session::ModelSelection>>>,
    model_factory: Option<ModelFactory>,
    fallback_model: model::DeepSeekConfig,
    fallback_credentials: Option<Arc<dyn model::CredentialSource>>,
    /// 任务执行者也只弱持有：做梦检查不能把任务库/效果泵的写者留到根宿主之后。
    task_runtime: Option<std::sync::Weak<TaskRuntime>>,
    /// 同时只跑一个。
    running: AtomicBool,
    /// 触发/检查的计数：测试用它等启动那次检查收尾，避免和回合结束的触发抢。
    checks_started: AtomicU64,
    checks_finished: AtomicU64,
    /// 根宿主装上的推送：做梦开始/结束让页面重读（seq=0 的 memory/changed，不走轮询）。
    changed_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl DreamRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        data_dir: &Path,
        cwd: &Path,
        memory: Arc<MemoryRuntime>,
        inner: Arc<tokio::sync::Mutex<Option<ServiceHost>>>,
        selection: Arc<Mutex<Option<session::ModelSelection>>>,
        model_factory: Option<ModelFactory>,
        fallback_model: model::DeepSeekConfig,
        fallback_credentials: Option<Arc<dyn model::CredentialSource>>,
        task_runtime: Option<Arc<TaskRuntime>>,
    ) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            cwd: cwd.to_path_buf(),
            memory,
            inner: std::sync::Arc::downgrade(&inner),
            selection,
            model_factory,
            fallback_model,
            fallback_credentials,
            task_runtime: task_runtime.map(|runtime| std::sync::Arc::downgrade(&runtime)),
            running: AtomicBool::new(false),
            checks_started: AtomicU64::new(0),
            checks_finished: AtomicU64::new(0),
            changed_hook: Mutex::new(None),
        }
    }

    /// 根宿主装上「记忆库变了」的推送；做梦开始/结束时调用它。
    pub fn set_changed_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self.changed_hook.lock().expect("changed hook 锁被毒化") = Some(hook);
    }

    fn notify_changed(&self) {
        let hook = self
            .changed_hook
            .lock()
            .expect("changed hook 锁被毒化")
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// 记一个时机：条件到才真跑；检查本身也异步做，不挡调用方。
    pub fn trigger(self: &Arc<Self>, trigger: DreamTrigger) {
        self.checks_started.fetch_add(1, Ordering::SeqCst);
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let _ = runtime.check(trigger).await;
        });
    }

    /// 用户说「整理一下」：正在跑回「正在整理」，不起第二个；没许可先出许可卡。
    pub fn request_now(self: &Arc<Self>) -> Result<String, String> {
        if self.running.load(Ordering::SeqCst) {
            return Ok("正在整理".to_owned());
        }
        let settings = self
            .memory
            .store()
            .dream_settings()
            .map_err(|error| error.to_string())?;
        if !settings.enabled {
            return Err("做梦开关关着".to_owned());
        }
        if !settings.permit {
            self.trigger(DreamTrigger::UserAsked);
            return Ok("要先允许观察才能整理；已经出了许可卡".to_owned());
        }
        self.trigger(DreamTrigger::UserAsked);
        Ok("开始整理".to_owned())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 一次检查；返回实际发生的事（测试与页面可能直接等它）。
    pub async fn check(self: &Arc<Self>, trigger: DreamTrigger) -> DreamCheck {
        if self.running.swap(true, Ordering::SeqCst) {
            // 早退也是一次检查的结束：不记的话 checks_settled 永远为 false，等启动检查会挂。
            self.checks_finished.fetch_add(1, Ordering::SeqCst);
            return DreamCheck::Running;
        }
        let outcome = self.check_inner(trigger).await;
        self.running.store(false, Ordering::SeqCst);
        self.checks_finished.fetch_add(1, Ordering::SeqCst);
        outcome
    }

    /// 之前触发的检查都收尾了（测试同步用；生产不用它等）。
    pub fn checks_settled(&self) -> bool {
        self.checks_started.load(Ordering::SeqCst) == self.checks_finished.load(Ordering::SeqCst)
    }

    async fn check_inner(self: &Arc<Self>, trigger: DreamTrigger) -> DreamCheck {
        let settings = match self.memory.store().dream_settings() {
            Ok(settings) => settings,
            Err(error) => return DreamCheck::Failed(error.to_string()),
        };
        if !settings.enabled {
            return DreamCheck::Skipped("做梦关着".to_owned());
        }
        let automatic = trigger != DreamTrigger::UserAsked;
        if automatic {
            // 回合结束的时机是在 turn/end 落日志时触发的，此时 Agent 还没翻成 idle：
            // 等它真的收尾再判忙，否则每一轮都会被自己挡掉。
            if trigger == DreamTrigger::TurnEnded {
                self.wait_root_idle().await;
            }
            if self.root_busy().await {
                return DreamCheck::Skipped("Kind 这一轮在跑或全局暂停".to_owned());
            }
            let now = self.memory.store().now();
            let interval_ms = i64::try_from(settings.interval_hours.saturating_mul(3_600_000))
                .unwrap_or(i64::MAX);
            // 失败也按间隔退一步，避免每一轮都重试。
            let last_mark = settings.last_success_at.max(settings.last_attempt_at);
            if now.saturating_sub(last_mark) < interval_ms {
                return DreamCheck::Skipped("距上次成功/尝试不到间隔".to_owned());
            }
            let new_events = self.new_event_count(settings.last_success_at);
            if new_events < settings.min_events {
                return DreamCheck::Skipped(format!("新回合/报回不到 {}", settings.min_events));
            }
            // 攒够了才要许可：没许可时出一张许可卡，用户允许前不读对话、不跑。
            if !settings.permit {
                self.ensure_permit_card().await;
                return DreamCheck::Skipped("没有观察许可".to_owned());
            }
        } else if !settings.permit {
            self.ensure_permit_card().await;
            return DreamCheck::Skipped("没有观察许可".to_owned());
        }
        match self.run_once().await {
            Ok(report) => DreamCheck::Ran(report),
            Err(error) => DreamCheck::Failed(error),
        }
    }

    /// 回合结束触发时等 Agent 真正收尾：先拿住根槽的强引用，再 idle().await。
    async fn wait_root_idle(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let agent = {
            let guard = inner.lock().await;
            guard.as_ref().map(|host| host.agent().clone())
        };
        if let Some(agent) = agent {
            agent.idle().await;
        }
    }

    /// Kind 这一轮在跑或全局暂停（D64）：自动时机跳过，等下一个时机。
    async fn root_busy(&self) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return true;
        };
        let guard = inner.lock().await;
        let running = guard.as_ref().is_some_and(|host| host.agent().is_running());
        drop(guard);
        let paused = self
            .task_runtime
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .is_some_and(|runtime| runtime.exec_paused());
        running || paused
    }

    fn root_events(&self) -> Vec<session::Event> {
        // 检查时只读一次会话日志；用 blocking_lock 会跨 await，这里用 try_lock 失败就空读。
        let Some(inner) = self.inner.upgrade() else {
            return Vec::new();
        };
        let Ok(guard) = inner.try_lock() else {
            return Vec::new();
        };
        guard
            .as_ref()
            .map(|host| host.agent().session_events_from(0))
            .unwrap_or_default()
    }

    /// 上次成功之后新的 Kind 回合 + 报回的任务条数。
    fn new_event_count(&self, since_ms: i64) -> u64 {
        let mut count = 0u64;
        for event in self.root_events() {
            if i64::try_from(event.time).unwrap_or(i64::MAX) < since_ms {
                continue;
            }
            match &event.kind {
                session::EventKind::TurnEnd { .. } => count += 1,
                session::EventKind::UserMessage { message, .. } if is_task_relay(message) => {
                    count += 1;
                }
                _ => {}
            }
        }
        count
    }

    async fn ensure_permit_card(&self) {
        let Ok(settings) = self.memory.store().dream_settings() else {
            return;
        };
        if settings.permit_card_at > 0 {
            return;
        }
        let now = self.memory.store().now();
        if self
            .append_card("permission", DREAM_PERMIT_CARD_REF)
            .await
            .is_ok()
            && let Err(error) =
                self.memory
                    .store()
                    .update_dream_settings(&memory::types::DreamSettingsPatch {
                        permit_card_at: Some(now),
                        ..Default::default()
                    })
        {
            eprintln!("做梦许可卡时间没记进库：{error}");
        }
    }

    /// 往根会话日志追加一张程序卡（id/kind/ref），页面从日志重建、实时广播。
    async fn append_card(&self, kind: &str, reference: &str) -> Result<String, String> {
        let now = self.memory.store().now();
        let serial = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        let id = format!("memory-card-{kind}-{now}-{serial}");
        let Some(inner) = self.inner.upgrade() else {
            return Err("根会话不可用".to_owned());
        };
        let guard = inner.lock().await;
        let host = guard.as_ref().ok_or_else(|| "根会话不可用".to_owned())?;
        host.append_event(session::EventKind::MemoryCard {
            id: id.clone(),
            kind: kind.to_owned(),
            reference: Some(reference.to_owned()),
        })
        .await
        .map_err(|error| error.to_string())?;
        Ok(id)
    }

    /// 最近一次成功做梦的 run；撤销只认它。
    pub fn latest_successful_run(&self) -> Result<Option<memory::types::DreamRun>, String> {
        let store = self.memory.store();
        let runs = store.dream_runs(20).map_err(|error| error.to_string())?;
        Ok(runs.into_iter().find(|run| run.outcome == "done"))
    }

    /// 撤销一处/整次；run_id 不给就撤最近一次成功的。用户从页面点，也允许做梦自己撤。
    pub fn undo_run(
        &self,
        run_id: Option<&str>,
        only: Option<&str>,
    ) -> Result<DreamReport, String> {
        let run = match run_id {
            Some(id) => self
                .memory
                .store()
                .dream_run(id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "没有这次做梦记录".to_owned())?,
            None => self
                .latest_successful_run()?
                .ok_or_else(|| "还没有整理完的做梦记录".to_owned())?,
        };
        if run.outcome != "done" {
            return Err("这次没整理完，没有能撤销的改动".to_owned());
        }
        let outcome = self
            .memory
            .store()
            .write(
                memory::types::WriteOp::DreamUndo {
                    run_id: run.id.clone(),
                    only: only.map(str::to_owned),
                },
                &memory::types::WriteContext::user(),
            )
            .map_err(|error| error.to_string())?;
        Ok(DreamReport {
            run_id: run.id,
            applied: outcome.dream_applied,
            skipped: outcome.dream_skipped,
        })
    }

    /// 撤销最近一次成功（工具/卡片「全部撤销」用）。
    pub fn undo(&self, only: Option<&str>) -> Result<DreamReport, String> {
        self.undo_run(None, only)
    }

    /// 跑一次：起一个独立的受限 agent 会话（Kind 当前模型），只读记忆/日记/对话/报回，
    /// 只写笔记；store 以 actor=Dream 再挡一次。
    async fn run_once(self: &Arc<Self>) -> Result<DreamReport, String> {
        let store = self.memory.store();
        let settings = store.dream_settings().map_err(|error| error.to_string())?;
        let root_events = self.root_events();
        let max_seq = self.root_events().last().map_or(0, |event| event.seq);
        let now = store.now();
        let run_id = format!("dream-{now}-{}", NEXT_RUN.fetch_add(1, Ordering::Relaxed));
        self.memory
            .store()
            .update_dream_settings(&memory::types::DreamSettingsPatch {
                last_attempt_at: Some(now),
                ..Default::default()
            })
            .map_err(|error| error.to_string())?;
        // 先把 run 行建出来：模型可能一处都不写、也可能中途失败；失败也要能记原因。
        store
            .write(
                memory::types::WriteOp::DreamApply {
                    run: memory::types::DreamRunInput {
                        id: run_id.clone(),
                        model: self.model_name(),
                        started_at: now,
                        ended_at: now,
                        outcome: "running".to_owned(),
                        report_json: "{}".to_owned(),
                        usage_json: "{}".to_owned(),
                    },
                    changes: Vec::new(),
                },
                &memory::types::WriteContext::dream(&run_id),
            )
            .map_err(|error| error.to_string())?;

        // 开始整理：页面靠推送重读，看到「正在整理」。
        self.notify_changed();
        let result = self.dream_agent_run(&run_id, &root_events, &settings).await;
        let ended = store.now();
        match result {
            Ok((session_id, usage_json)) => {
                let changes = store
                    .dream_changes(&run_id)
                    .map_err(|error| error.to_string())?;
                let report = json!({
                    "run_id": run_id,
                    "session_id": session_id,
                    "applied": changes.len(),
                    "skipped": Vec::<String>::new(),
                    "max_seq": max_seq,
                })
                .to_string();
                store
                    .finish_dream_run(&run_id, ended, "done", &report, &usage_json, "")
                    .map_err(|error| error.to_string())?;
                store
                    .update_dream_settings(&memory::types::DreamSettingsPatch {
                        last_success_at: Some(ended),
                        last_read_seq: Some(max_seq),
                        ..Default::default()
                    })
                    .map_err(|error| error.to_string())?;
                if let Err(error) = self.append_card("dream_done", &run_id).await {
                    eprintln!("做梦完成卡没写进根会话：{error}");
                }
                // 结束：页面靠推送重读，看到「整理完了」和这次的改动。
                self.notify_changed();
                Ok(DreamReport {
                    run_id,
                    applied: changes.len(),
                    skipped: Vec::new(),
                })
            }
            Err(error) => {
                let report = json!({ "run_id": run_id, "error": error }).to_string();
                if let Err(write) =
                    store.finish_dream_run(&run_id, ended, "failed", &report, "{}", &error)
                {
                    eprintln!("做梦失败记录没写进库：{write}");
                }
                if let Err(card) = self.append_card("dream_failed", &run_id).await {
                    eprintln!("做梦失败卡没写进根会话：{card}");
                }
                self.notify_changed();
                Err(error)
            }
        }
    }

    fn model_name(&self) -> String {
        self.selection
            .lock()
            .expect("selection 锁被毒化")
            .as_ref()
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| self.fallback_model.model.clone())
    }

    fn resolve_model(&self) -> Result<DreamModel, String> {
        let selection = self.selection.lock().expect("selection 锁被毒化").clone();
        if let Some(factory) = self.model_factory.as_ref() {
            let (_, config, credentials) = factory(selection.as_ref())?;
            let name = config.model.clone();
            return Ok((config, credentials, name));
        }
        Ok((
            self.fallback_model.clone(),
            self.fallback_credentials.clone(),
            self.fallback_model.model.clone(),
        ))
    }

    async fn dream_agent_run(
        self: &Arc<Self>,
        run_id: &str,
        root_events: &[session::Event],
        settings: &memory::types::DreamSettings,
    ) -> Result<(String, String), String> {
        let (model_config, credentials, _) = self.resolve_model()?;
        let Some(credentials) = credentials else {
            return Err("没有可用的凭证".to_owned());
        };
        let config = agent::AgentConfig {
            instruction_roots: Vec::new(),
            skill_roots: Vec::new(),
            bundled_skills: Vec::new(),
            memory_context: None,
            pre_compaction: None,
            ..agent::AgentConfig::default()
        };
        let invalid_exhausted = Arc::new(AtomicBool::new(false));
        let extra_tools = self.dream_tools(run_id, settings, Arc::clone(&invalid_exhausted));
        let agent = agent::Agent::open(agent::AgentOpenOptions {
            data_dir: self.data_dir.clone(),
            cwd: self.cwd.clone(),
            resume: None,
            model: model_config,
            config,
            credentials: Some(credentials),
            append_hook_factory: None,
            extra_tools,
            tool_policy: agent::ToolPolicy {
                only: Some(
                    [
                        "memory_search",
                        "memory_read",
                        "memory_conversation_read",
                        "memory_task_reports",
                        "memory_dream_apply",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                ),
                no_wait: false,
            },
            policy_prompt: Some(DREAM_POLICY.to_owned()),
        })
        .await
        .map_err(|error| error.to_string())?;
        let session_id = agent.id().as_str().to_owned();
        let host = ServiceHost::new(agent);
        let prompt = initial_prompt(settings, root_events);
        host.send(service_send(&format!("dream-run:{run_id}"), prompt))
            .await
            .map_err(|error| error.to_string())?;
        let idle =
            tokio::time::timeout(std::time::Duration::from_secs(300), host.agent().idle()).await;
        let events = host.agent().session_events_from(0);
        let usage_json = sum_usage(&events);
        host.shutdown().await;
        idle.map_err(|_| "做梦超时".to_owned())?;
        // 模型没走完/出错时，会话里会有 turn/end error：当成失败，让失败卡写原因。
        if let Some(error) = turn_failure(&events) {
            return Err(error);
        }
        if invalid_exhausted.load(Ordering::SeqCst) {
            return Err("做梦改动连续 3 次没过校验，保留上一版".to_owned());
        }
        Ok((session_id, usage_json))
    }

    fn dream_tools(
        self: &Arc<Self>,
        run_id: &str,
        settings: &memory::types::DreamSettings,
        invalid_exhausted: Arc<AtomicBool>,
    ) -> Vec<Arc<dyn Tool>> {
        let window = DreamWindow::of(settings);
        let mut tools: Vec<Arc<dyn Tool>> =
            super::memory_tools(Arc::clone(&self.memory), &self.data_dir, None, None)
                .into_iter()
                .filter(|tool| tool.name() == "memory_read")
                .collect();
        tools.push(Arc::new(DreamSearchTool {
            runtime: Arc::clone(self),
            window,
        }));
        tools.push(Arc::new(ConversationReadTool {
            runtime: Arc::clone(self),
            window,
        }));
        tools.push(Arc::new(TaskReportsTool {
            runtime: Arc::clone(self),
            window,
        }));
        tools.push(Arc::new(DreamApplyTool {
            runtime: Arc::clone(self),
            run_id: run_id.to_owned(),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            exhausted: invalid_exhausted,
        }));
        tools
    }
}

fn service_send(request_id: &str, content: String) -> crate::protocol::SendRequest {
    crate::protocol::SendRequest {
        request_id: request_id.to_owned(),
        content,
        delivery: crate::protocol::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        object_refs: Vec::new(),
        quotes: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
        source: None,
    }
}

fn is_task_relay(message: &model::Message) -> bool {
    matches!(
        &message.source,
        model::MessageSource::User { rpc_id: Some(rpc_id) }
            if crate::program_messages::is_task_relay_rpc(rpc_id)
    )
}

fn message_text(message: &model::Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn sum_usage(events: &[session::Event]) -> String {
    let mut input = 0u64;
    let mut output = 0u64;
    for event in events {
        if let session::EventKind::AssistantMessage {
            usage: Some(usage), ..
        } = &event.kind
        {
            input = input.saturating_add(usage.input_tokens.unwrap_or(0));
            output = output.saturating_add(usage.output_tokens.unwrap_or(0));
        }
    }
    json!({"inputTokens": input, "outputTokens": output}).to_string()
}

fn turn_failure(events: &[session::Event]) -> Option<String> {
    for event in events.iter().rev() {
        if let session::EventKind::TurnEnd { reason, .. } = &event.kind
            && let session::TurnEndReason::Error { error } = reason
        {
            return Some(
                format!("{} {}", error.code, error.message)
                    .trim()
                    .to_owned(),
            );
        }
    }
    None
}

fn initial_prompt(
    settings: &memory::types::DreamSettings,
    root_events: &[session::Event],
) -> String {
    let max_seq = root_events.last().map_or(0, |event| event.seq);
    format!(
        "这次整理的窗口：上次成功时间 {}、上次读到的 Kind 对话 seq {}；现在根会话 seq 到 {}。\n先用工具看这段时间的 Kind 对话、日记和任务报回，再决定要不要改你的笔记；没有值得留下的就直说这次没什么可整理。",
        settings.last_success_at, settings.last_read_seq, max_seq
    )
}

const DREAM_POLICY: &str = concat!(
    "You are AIOS's dream helper: a restricted background agent that tidies Kind's own notes. ",
    "You can only read memory, diary, the Kind conversation and task reports; you never read web pages or files. ",
    "\nYou may only write Kind's own notes (note layer). Entries, about-you and diary are read-only: never try to change them. ",
    "\nMerge duplicates, correct wrong notes, retire stale notes, add new observations from failures, and write a \"next time do it differently\" note when the user or a verdict taught a lesson. ",
    "\nEvery change must carry a short reason. Submit changes with memory_dream_apply; if nothing is worth keeping, submit nothing and say so in one sentence.",
    "\nOnly write a note when it is worth keeping on its own: a stable preference, a reusable lesson, a fact that will still matter next month; do not copy one-off task chatter.",
    "\nTurn relative time in the source (tomorrow, last week, 3 days ago) into an absolute date before writing it down.",
    "\nIf memory_dream_apply returns a problem code, rewrite only that change once; after three rejected attempts the run keeps the previous version and fails.",
    "\nWhen correcting an existing note, set before_revision to the revision you read; when retiring one, list its source refs in dropped_sources instead of silently dropping them.",
);

// ---- 做梦专用工具 ----

struct ConversationReadTool {
    runtime: Arc<DreamRuntime>,
    window: DreamWindow,
}

impl Tool for ConversationReadTool {
    fn name(&self) -> &str {
        "memory_conversation_read"
    }

    fn description(&self) -> &str {
        "Read the current Kind conversation (user words and Kind answers only), newest window first. Use after_seq to continue from a seq."
    }

    fn parameters(&self) -> &Value {
        static PARAMETERS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMETERS.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "after_seq": {"type": "integer", "description": "Only events with seq greater than this."},
                    "limit": {"type": "integer", "description": "Max events, default 60."}
                }
            })
        })
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            let after_seq = clamp_after_seq(args["after_seq"].as_u64(), self.window.after_seq);
            let limit = args["limit"].as_u64().unwrap_or(60).clamp(1, 200) as usize;
            let lines = conversation_window_lines(&self.runtime.root_events(), after_seq, limit);
            if lines.is_empty() {
                return ToolResult::success(call.id, "这个窗口没有对话。");
            }
            ToolResult::success(
                call.id,
                lines.join(
                    "
",
                ),
            )
        })
    }
}

fn is_program_prefix(rpc_id: &str) -> bool {
    crate::program_messages::is_program_rpc(rpc_id)
}

/// 这次做梦的读窗口：上次成功时间和上次读到的 seq 之前的东西不读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DreamWindow {
    after_seq: u64,
    after_ms: i64,
}

impl DreamWindow {
    fn of(settings: &memory::types::DreamSettings) -> Self {
        Self {
            after_seq: settings.last_read_seq,
            after_ms: settings.last_success_at,
        }
    }
}

/// 工具参数只能把窗口往里收，不能往外放：不传或传 0 都按下限。
fn clamp_after_seq(requested: Option<u64>, floor: u64) -> u64 {
    requested.unwrap_or(floor).max(floor)
}

fn clamp_after_ms(requested: Option<i64>, floor: i64) -> i64 {
    requested.unwrap_or(floor).max(floor)
}

/// 窗口内的对话：用户自己的话和 Kind 的回答，按 seq 排好；程序消息不读。
fn conversation_window_lines(
    events: &[session::Event],
    after_seq: u64,
    limit: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    for event in events {
        if event.seq <= after_seq {
            continue;
        }
        match &event.kind {
            session::EventKind::UserMessage { message, .. } => {
                let text = message_text(message);
                if text.trim().is_empty() {
                    continue;
                }
                let read = match &message.source {
                    model::MessageSource::User { rpc_id } => {
                        !rpc_id.as_deref().is_some_and(is_program_prefix)
                    }
                    _ => false,
                };
                if read {
                    lines.push((event.seq, format!("[{}] 用户：{}", event.seq, text)));
                }
            }
            session::EventKind::AssistantMessage { message, .. } => {
                if message.role != model::Role::Assistant {
                    continue;
                }
                let text = super::assistant_answer_text(message);
                if text.trim().is_empty() {
                    continue;
                }
                lines.push((event.seq, format!("[{}] Kind：{}", event.seq, text)));
            }
            _ => {}
        }
        if lines.len() >= limit {
            break;
        }
    }
    lines.sort_by_key(|(seq, _)| *seq);
    lines.into_iter().map(|(_, line)| line).collect()
}

/// 搜索命中也要过窗口：只留窗口之后的对话命中（记忆和日记不限）。
fn window_search_hits(
    hits: Vec<memory::types::SearchHit>,
    after_seq: u64,
) -> Vec<memory::types::SearchHit> {
    hits.into_iter()
        .filter(|hit| match hit {
            memory::types::SearchHit::Conversation { seq, .. } => *seq > after_seq,
            _ => true,
        })
        .collect()
}

/// 做梦专用搜索：和常规搜索同一份实现，只多一道窗口。
struct DreamSearchTool {
    runtime: Arc<DreamRuntime>,
    window: DreamWindow,
}

impl Tool for DreamSearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search memory, diary and the Kind conversation since the last dream. Conversation hits are limited to the dream window."
    }

    fn parameters(&self) -> &Value {
        static PARAMETERS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMETERS.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to search for."}
                },
                "required": ["query"]
            })
        })
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            let query = args["query"].as_str().unwrap_or("").trim().to_owned();
            if query.is_empty() {
                return ToolResult::error(call.id, ToolFailure::plain("缺少 query".to_owned()));
            }
            let hits = match self.runtime.memory.store().search(&query, 20) {
                Ok(hits) => window_search_hits(hits, self.window.after_seq),
                Err(error) => {
                    return ToolResult::error(call.id, ToolFailure::plain(error.to_string()));
                }
            };
            if hits.is_empty() {
                return ToolResult::success(call.id, "窗口里没有搜到。");
            }
            let mut lines = Vec::new();
            for hit in hits {
                match hit {
                    memory::types::SearchHit::Memory(item) => {
                        lines.push(format!("[记忆 {}] {}：{}", item.id, item.title, item.body))
                    }
                    memory::types::SearchHit::Diary(entry) => {
                        lines.push(format!("[日记 {}] {}", entry.id, entry.text))
                    }
                    memory::types::SearchHit::Conversation { seq, text, .. } => {
                        lines.push(format!("[对话 {seq}] {text}"))
                    }
                }
            }
            ToolResult::success(
                call.id,
                lines.join(
                    "
",
                ),
            )
        })
    }
}

struct TaskReportsTool {
    runtime: Arc<DreamRuntime>,
    window: DreamWindow,
}

impl Tool for TaskReportsTool {
    fn name(&self) -> &str {
        "memory_task_reports"
    }

    fn description(&self) -> &str {
        "Read task reports, gate verdicts and requirement changes since the last dream; optional task_id to read one task."
    }

    fn parameters(&self) -> &Value {
        static PARAMETERS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMETERS.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "after_ms": {"type": "integer", "description": "Only facts at or after this unix-ms time; default 0."},
                    "task_id": {"type": "string", "description": "Read one task only."}
                }
            })
        })
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let Some(task_runtime) = self
                .runtime
                .task_runtime
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
            else {
                return ToolResult::success(call.id, "没有任务库。");
            };
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            let after_ms = clamp_after_ms(args["after_ms"].as_i64(), self.window.after_ms);
            let task_id = args["task_id"].as_str().map(str::to_owned);
            match task_facts(&task_runtime, after_ms) {
                Ok(facts) => {
                    let mut lines = Vec::new();
                    for fact in facts {
                        if task_id
                            .as_deref()
                            .is_some_and(|wanted| wanted != fact.task_id)
                        {
                            continue;
                        }
                        lines.push(format!(
                            "任务 {}「{}」要求：{}",
                            fact.task_id, fact.title, fact.requirement
                        ));
                        lines.extend(fact.facts.iter().map(|line| format!("- {line}")));
                    }
                    if lines.is_empty() {
                        ToolResult::success(call.id, "这个窗口没有报回的任务。")
                    } else {
                        ToolResult::success(
                            call.id,
                            lines.join(
                                "
",
                            ),
                        )
                    }
                }
                Err(error) => ToolResult::error(call.id, ToolFailure::plain(error)),
            }
        })
    }
}

struct DreamApplyTool {
    runtime: Arc<DreamRuntime>,
    run_id: String,
    attempts: std::sync::atomic::AtomicUsize,
    exhausted: Arc<AtomicBool>,
}

impl DreamApplyTool {
    fn failed_attempt(&self, call: ToolCall, code: &str, message: &str) -> ToolResult {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt >= 3 {
            self.exhausted.store(true, Ordering::SeqCst);
            return ToolResult::error(
                call.id,
                ToolFailure::plain(format!(
                    "{code}：{message}；连续 3 次没过校验，这一版不写，保留上一版"
                )),
            );
        }
        ToolResult::error(
            call.id,
            ToolFailure::plain(format!(
                "{code}：{message}；只重写这一处（第 {attempt}/3 次）"
            )),
        )
    }
}

impl Tool for DreamApplyTool {
    fn name(&self) -> &str {
        "memory_dream_apply"
    }

    fn description(&self) -> &str {
        "Apply your note changes. Actions: merge (merge into an existing note), correct (fix a note), retire (drop a stale note), new_note (add a note). Only note-layer targets are accepted. Keep before_revision from what you read; retire must list the source refs it drops in dropped_sources."
    }

    fn parameters(&self) -> &Value {
        static PARAMETERS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMETERS.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "changes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "memory_id": {"type": "string", "description": "Existing note id for merge/correct/retire; omit for new_note."},
                                "action": {"type": "string", "enum": ["merge", "correct", "retire", "new_note"]},
                                "title": {"type": "string"},
                                "summary": {"type": "string"},
                                "body": {"type": "string"},
                                "reason": {"type": "string"},
                                "before_revision": {"type": "integer", "description": "The revision you read; mismatch is rejected."},
                                "dropped_sources": {"type": "array", "items": {"type": "string"}, "description": "Source refs you intend to drop; list them explicitly."}
                            },
                            "required": ["action", "reason"]
                        }
                    }
                },
                "required": ["changes"]
            })
        })
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args: Value = match serde_json::from_str(&call.arguments) {
                Ok(value) => value,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("参数不是 JSON 对象：{error}")),
                    );
                }
            };
            let Some(items) = args["changes"].as_array() else {
                return ToolResult::error(call.id, ToolFailure::plain("changes 必须是数组"));
            };
            let mut changes = Vec::new();
            for item in items {
                let action = match item["action"].as_str().unwrap_or_default() {
                    "merge" => memory::types::DreamAction::Merge,
                    "correct" => memory::types::DreamAction::Correct,
                    "retire" => memory::types::DreamAction::Retire,
                    "new_note" => memory::types::DreamAction::NewNote,
                    other => {
                        return self.failed_attempt(
                            call,
                            "invalid_op",
                            &format!("不认识的 action：{other}"),
                        );
                    }
                };
                let memory_id = item["memory_id"].as_str().map(str::to_owned);
                let title = item["title"].as_str().unwrap_or("").trim().to_owned();
                let body = item["body"].as_str().unwrap_or("").trim().to_owned();
                if memory_id.is_none() && (title.is_empty() || body.is_empty()) {
                    return self.failed_attempt(call, "invalid_op", "新记一条要 title 和 body");
                }
                changes.push(memory::types::DreamChangeInput {
                    memory_id,
                    title,
                    summary: item["summary"].as_str().unwrap_or("").trim().to_owned(),
                    body,
                    action,
                    reason: item["reason"].as_str().unwrap_or("").trim().to_owned(),
                    before_revision: item["before_revision"].as_u64(),
                    dropped_sources: item["dropped_sources"]
                        .as_array()
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|value| value.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
            if changes.is_empty() {
                return ToolResult::success(call.id, "没有改动。");
            }
            let now = self.runtime.memory.store().now();
            match self.runtime.memory.store().write(
                memory::types::WriteOp::DreamApply {
                    run: memory::types::DreamRunInput {
                        id: self.run_id.clone(),
                        model: self.runtime.model_name(),
                        started_at: now,
                        ended_at: now,
                        outcome: "running".to_owned(),
                        report_json: "{}".to_owned(),
                        usage_json: "{}".to_owned(),
                    },
                    changes,
                },
                &memory::types::WriteContext::dream(&self.run_id),
            ) {
                Ok(outcome) => {
                    self.exhausted.store(false, Ordering::SeqCst);
                    ToolResult::success(
                        call.id,
                        format!("应用了 {} 处笔记改动。", outcome.dream_applied),
                    )
                }
                Err(error) => {
                    if let memory::MemoryError::DreamValidation(validation) = &error {
                        self.failed_attempt(call, validation.code, &validation.message.clone())
                    } else {
                        ToolResult::error(call.id, ToolFailure::plain(error.to_string()))
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_event(seq: u64, text: &str, rpc: &str) -> session::Event {
        session::Event::new(
            seq,
            seq,
            session::EventKind::UserMessage {
                turn: seq,
                step: 1,
                message: model::Message::user_text_with_request_id(text, rpc),
            },
        )
    }

    fn assistant_event(seq: u64, text: &str) -> session::Event {
        session::Event::new(
            seq,
            seq,
            session::EventKind::AssistantMessage {
                turn: seq,
                step: 1,
                message: model::Message::assistant(
                    vec![model::ContentBlock::text(text)],
                    "deepseek",
                    "deepseek-flash",
                ),
                stream: Vec::new(),
                usage: None,
                interrupted: None,
                effort: None,
                effort_wanted: None,

                request_at: None,
                context_parts: None,
            },
        )
    }

    #[test]
    fn window_floors_ignore_older_arguments() {
        assert_eq!(clamp_after_seq(None, 12), 12);
        assert_eq!(clamp_after_seq(Some(0), 12), 12);
        assert_eq!(clamp_after_seq(Some(20), 12), 20);
        assert_eq!(clamp_after_ms(None, 1_700), 1_700);
        assert_eq!(clamp_after_ms(Some(0), 1_700), 1_700);
        assert_eq!(clamp_after_ms(Some(2_000), 1_700), 2_000);
    }

    #[test]
    fn conversation_window_excludes_old_events_and_program_messages() {
        let events = vec![
            user_event(2, "旧的一轮：咖啡要双份", "old-user"),
            assistant_event(3, "旧回答：知道了"),
            user_event(5, "中继卡里的摘要", "task-report:task-1:report-1"),
            assistant_event(6, "新回答：茶要热的"),
            user_event(7, "新的一轮：茶要热的", "new-user"),
        ];
        let lines = conversation_window_lines(&events, 3, 60);
        assert_eq!(lines.len(), 2, "只留窗口后的用户话和 Kind 回答：{lines:?}");
        assert!(lines.iter().any(|line| line.contains("新的一轮")));
        assert!(lines.iter().any(|line| line.contains("新回答")));
        assert!(!lines.iter().any(|line| line.contains("旧的一轮")));
        assert!(!lines.iter().any(|line| line.contains("旧回答")));
        assert!(!lines.iter().any(|line| line.contains("中继卡")));
    }

    #[test]
    fn search_hits_are_limited_to_the_window() {
        let hits = vec![
            memory::types::SearchHit::Conversation {
                conversation_id: "conv".to_owned(),
                seq: 2,
                turn: 1,
                text: "旧的一轮：咖啡要双份".to_owned(),
            },
            memory::types::SearchHit::Conversation {
                conversation_id: "conv".to_owned(),
                seq: 7,
                turn: 3,
                text: "新的一轮：茶要热的".to_owned(),
            },
        ];
        let kept = window_search_hits(hits, 3);
        assert_eq!(kept.len(), 1, "只留窗口后的命中：{kept:?}");
        assert!(matches!(
            &kept[0],
            memory::types::SearchHit::Conversation { seq: 7, .. }
        ));
    }
}
