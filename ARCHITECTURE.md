# AIOS 架构

- 本文件只写不常变的东西：代码地图、依赖方向、不变量、接缝。
- 产品与设计文档在 docs/（不提交）；这里不重复。

## 鸟瞰

AIOS 是一个本地 agent 内核：每次模型请求从会话日志派生，工具在本地执行，界面后续接 Tauri。
当前仓库只有第一个内核切片（T004 agent loop）的骨架。

## 代码地图

- crates/model：provider 无关的消息 IR（Message / ContentBlock / Usage / FinishReason / StreamChunk；assistant 的 reasoning 块带 signature），DeepSeek 的 Messages 适配器（请求序列化、SSE 解码、流事件翻译、错误码、retry-after），以及具体类型 `CancelToken`（Arc<AtomicBool>+Notify，供 LLM 层打断请求）。
- crates/session：会话事件（turn/step/消息 + `compaction/prune` 的修剪视图事件 + `todo/write` 的整表 todo 快照事件 + `compaction/start` / `compaction/summary` / `compaction/end` 的压缩事件），只追加的 JSONL 存储（SessionStore），从日志派生请求消息（应用修剪替换视图），以及续接时的中断修复（`repair`：崩溃/被杀后给没收尾的 turn 补收尾事件）。
- crates/tools：Tool 接口、ToolCall/ToolResult、ToolSet、工具批执行（独占屏障、有界并行池、按模型顺序提交），内置工具：只读 read / glob / grep（进程内搜索，不起 rg 子进程）和写类 write / edit / apply_patch（整文件写、字面替换、Codex 风格补丁；文件系统语义在 `crates/tools/src/fs`），大工具结果的 spill 存储与保留（`spill`：按会话目录落盘、头/尾 preview + locator），通用 bash 工具（每次新 `bash -c`、超时转后台、输出尾部截断 + 全文 spill 路径、受管 `AIOS_*` 环境变量），后台任务三件 job_output / job_list / job_kill 与按会话隔离的任务注册表（`jobs`：子进程流式收集、超时转后台、结算回调），skill 发现与 `skill` 加载工具（`skills`：项目根/.agents/skills、<data-dir>/skills、配置根；最小 YAML frontmatter 解析），todo 工具 `todo_write`（整表替换 + 校验 + 计数回执；落日志由 agent 负责），工具级限时超时（`batch`：`Tool::timeout_ms()` 声明，超时回 `TOOL_TIMEOUT` 错误结果）、tools 为此新增 workspace tokio 生产依赖，只用于 `tokio::time::timeout`（不创建 runtime、不 spawn）、以及工具用的会话上下文 `ToolContext`（cwd + 观察表 / 任务注册表 / spill，见「T005 接口地基」）。
- crates/agent：一轮多步的驱动、收件箱（follow-up / steer / inject）、工具调用的派发与结果回写、Agent 的 6 个驱动入口（create / resume / send / cancel / subscribe / idle）、公开配置 `AgentConfig`、订阅者事件 `AgentEvent`、后台任务完成投递（结算时发 `JobCompleted` 并把完成通知投进下一步输入）、重复工具调用提醒（同工具+规范化参数连续 3/5/8 次按 DSH 文案提醒）、指令文件注入（`instructions`：AGENTS.md / CLAUDE.md 的发现、按步注入与刷新）、skill 目录注入（`skill_catalog`：名字 + description 的目录块，按步追加）、自动压缩与溢出恢复与手动 `compact()` 入口（`compaction`：token 阈值检查、摘要请求、压缩事件、重试一次；空闲立刻压缩、运行中排到本轮之后）；另有 id / log_path 两个只读元数据访问器，不属于驱动入口。集成测试的 mock 响应器、可控测试工具和公共 setup 在 `crates/agent/tests/common`。loop 的固定挂点与功能注册表在 `crates/agent/src/capabilities.rs`（T006.1a：编译期 trait + 固定顺序表，无运行时加载）。
- crates/cli：开发用 CLI `aios-dev`（`chat` / `run`）；dev 覆盖参数（--context-window/--max-tokens/--compaction-threshold/--compaction-retain）仅开发用、生产默认不变。

## 依赖方向

`cli → agent → tools → session → model`

- 下层不依赖上层。
- 跨层只走各 crate 的公开接口。

## 不变量

- **会话日志只追加**：`SessionStore::append` 是唯一追加事件的入口；`create` 写首行 header、`open` 只截掉未提交的尾行，不重写已提交内容。每批事件先写入再 fsync，写失败回滚到写之前的长度。
- **每次模型请求都从会话日志派生**：不保留一份会话内的独立消息副本。
- **公开接口上只有 `tools::Tool` 是 trait**：其他公开接缝用具体类型（`agent::Deps { model, tools, store }`）。`session` 的文件写入接缝 `LogWriter` 是内部接缝（生产用 `std::fs::File`，测试用它注入 fsync 失败），不暴露在公开接口上。`model::CancelToken` 与 `DeepSeek::stream` 的取消参数也是具体类型，不新增 trait。测试替身放在 HTTP 层（wiremock）和临时目录上，不越过公开接口去 mock 内部类型。
- **工具批执行按模型顺序提交**：独占调用是屏障；连续的并行安全调用进有界滚动池（`max_parallel_tool_calls`，默认 10，设成 1 就是串行）；结果和 `tool/result` 事件都按模型给出的顺序，与完成顺序无关。
- **工具调用先记日志再派发**：`tool/call` 在工具执行前写入；`tool/result` 按模型顺序写入。取消与中断修复（T004.3 / T004.4）依赖这个顺序。
- **编辑前先读**：read 成功时按 realpath targetKey 记版本，读确认缺失时记 absent；write 对未见/确认缺失的目标走 createIfAbsent、对读过且版本未变的目标走 replaceIfVersion；edit 与 apply_patch 对没读过的目标返回 `FS_NOT_OBSERVED`、读过之后被外部改过的返回 `FS_STALE_VERSION`。观察状态只在会话内，续接时清空（照 DSH `fs-observation-policy`）。
- **写类工具独占**：write / edit / apply_patch 的并发类别是 `Exclusive`，在批执行里形成屏障；文件写入走同目录 temp → fsync → rename（createIfAbsent 用 temp + hard-link no-replace），失败清理 temp。
- **大工具结果先 spill 再进上下文**：工具结果估计 token 超过 `AgentConfig.spill_max_inline_tokens`（默认 12500；0 = 禁用）时，完整文本写进 `<data-dir>/spill/session-<id>/`，日志与派生请求只放头/尾 preview + locator（按会话隔离）；read 工具不参与（自带分页上限，照 DSH `spill-policy`）。写 spill 失败时保守保留原文。
- **旧大结果在派生请求里修剪**：派生之前，上一个 turn 及更早的、超过 8192 码点的 tool/result 换成 head 4096 + `[... tool result middle pruned ...]` + tail 1024 的视图，并追加 `compaction/prune` 事件；原始 tool/result 保留在日志里可回放，同一 call_id 只修剪一次（T005.2；修剪触发与 T005.8a 的摘要压缩无关）。
- **ask_user 问题与回答**：模型调用内置 ask_user_question 时 driver 不执行工具，而是发 AgentEvent::QuestionAsked（携带全部 questions，T005.12）+ 每个问题一条日志 question/asked；run_step 停住等 Agent::answer(question_id, text) 按 id 累积，全部答完后把答案按问题顺序写成 DSH 形态 tool/result 的 answers[{id, answer}] 续上、同一轮继续；未答完不生成 tool/result。任一问题参数不合法整单拒绝（ASK_USER_INVALID），结果走统一 tool/result 提交路径（不绕过 todo/repeat/abort）。等待中 cancel() 对未答问题各发 question/cancelled 并 aborted 收尾，无悬挂等待。CLI chat 逐题打印并读一行作答；非交互 run 打印全部问题后取消本轮（不静默卡死）。
- **重复调用提醒与工具级超时**：driver 在一次工具批后按「工具名 + JSON 深度 key 排序后的参数」维护连续重复链；命中 DSH 默认阈值 3/5/8 时在工具结果后追加一条提醒 user 消息（第 3 次温和、第 5/8 次详细；`todo_write` 不跟踪；每 step 最多一条；`send` 新用户消息清零计数）。`Tool::timeout_ms()` 默认 None，兼容现有工具；声明了限时的工具在批执行里用 tokio timeout 包裹，超时返回 `Error: tool call timed out after <ms>ms` + `TOOL_TIMEOUT` 错误结果，本轮继续（与 T005.4 的命令级「超时转后台」是两层）。
- **指令文件按步注入**：用户全局链是 `<data-dir>/AGENTS.md` 再按 `AgentConfig::instruction_roots` 的顺序读各根 `AGENTS.md`（默认 `~/.agents`，**不读 `~/.dsh`**）；项目链从 `.git` 根到 cwd，每个目录候选顺序 `AGENTS.md`、`CLAUDE.md`、`AGENTS.local.md`、`CLAUDE.local.md`。每个 step 组装请求前发现/渲染，有变化就以普通 `user/message` 追加（首次=基线，之后=刷新），位置紧随已领取消息；同一候选组内去首尾空白后内容相同的文件只渲染一次；每条链 65,536 字节预算（超了先省宽泛文件、再截断最具体文件并给 notice）。更深目录来自 T005.1 的路径观察（read / write / edit / apply_patch；glob 不记观察）；续接从日志里既有的 `Instructions from:` 段对账，不重复注入基线（T005.5）。
- **skill 目录与加载**：skill 根按优先级 = 项目根 `.agents/skills` → `<data-dir>/skills` → `AgentConfig::skill_roots`（默认 `~/.agents/skills`，**不读 `~/.dsh`**）；每个根只看顶层目录 bundle `<name>/SKILL.md` 或平铺 `<name>.md`，同名时先出现的根胜出。frontmatter 用最小 YAML 子集解析（不引 YAML 依赖）：必填 `name`（kebab-case）与 `description`，缺失/坏掉/名字非法的 skill 静默丢弃（工具也拒绝加载）。每个 step 组装请求前发现一次，有变化时把「名字 + description（≤500 字符）」以 user 角色的 `<system-reminder>` 目录块追加（`<available_skills>`，只含摘要不含正文）；模型用 `skill` 工具按精确名字加载全文，返回 `<skill_content>` 块 + 来源目录。续接从日志里既有的目录消息对账，不重复注入（T005.6）。
- **自动压缩与溢出恢复**：每个请求前用 DSH token 估算（chars/4+4，含消息与工具 schema）检查预算：阈值 = `floor(min(W×thresholdRatio, (W−O)−headroomTokens))`，保留预算 = `floor((W−O)×retainRatio)`（W 取自 `DeepSeekConfig.context_window`，O 是请求输出上限）。越过阈值时把较早历史（保留系统消息与近期 tail，不拆 tool-call/result）用一次额外模型请求生成摘要，追加 `compaction/start` + `compaction/summary`（摘要 + 被替换事件 seq 区间 + 估算 token 数）+ `compaction/end`，并发出 `CompactionStarted`/`CompactionEnded` 事件；原始事件保留在日志里，派生请求由一条 checkpoint 用户消息替换该区间（同一条日志只保留最后一次压缩的 checkpoint）。模型返回 `CONTEXT_WINDOW_EXCEEDED`（HTTP 400/413 且错误类型/文案匹配）时先强制压缩再重试一次，第二次溢出按正常错误收尾；自动压缩失败不打断本轮（事件记录 error）。连续两次相同失败不重试。手动入口 `Agent::compact()`（chat 的 `/compact`）：空闲时立刻压缩，运行中排队到当前轮之后（kick 在轮次边界处理队列并唤醒等待者）；compaction_id 带进程/纳秒/计数器成分，续接或新进程不会与旧 id 重复。`FORMAT_VERSION` 仍为 1（增补事件类型）。
- **todo 整表替换、日志承载状态**：`todo_write` 每次提交完整列表，条目 `{content, status}`（status 三态 pending/in_progress/completed）；校验：trim 后非空、content 去重、item 不得有未知键、空列表等于清空；DSH 的 `allowParallelInProgress` 是部署配置，AIOS 没有配置面，固定允许多个 in_progress（偏离记 handback）。工具只把快照写进 `ToolSessionState`，由 agent 在成功的工具批之后追加 `todo/write { turn, step, todos }` 事件；回放时后写覆盖先写，`Agent::resume` 从日志恢复当前列表。模型不需要额外注入消息：列表就在自己之前的 `todo_write` 调用参数里（DSH 语义）。
- **bash 每次新进程**：每次调用起新的 `bash -c`（不持久化）；stdout/stderr 超过 64,000 字节只留尾部、完整流写进会话 spill 目录并在结果里给路径；非零退出是结果（`[exit code: N]` marker），不是错误；超时未结束的命令留在后台当 job（返回 job id），不杀进程；父环境的 KEY/PASSWORD/SECRET/TOKEN（大小写不敏感）与 DSH_*/AIOS_* 变量不传给子进程、再注入受管 AIOS_*（挡凭据泄漏，照 DSH scrubbedParentEnv）；不做沙箱（D38），工具描述不宣称受限（T005.3）。
- **后台任务按会话隔离、跨 turn 存续**：任务注册表挂在 `ToolSessionState`，id 为 `bash-<pid>-<nanos>-<n>`；输出由读线程流式落到 `<data-dir>/jobs/session-<id>/`（目录 0700 / 文件 0600，每 job truncate 新建；续接旧 data-dir 时启动即修正旧目录/文件权限、旧文件保留归档）并在内存保留尾部；前台 bash 在 `start(..., foreground)` 时就标记 awaited，结算时若已被前台等住就不再发通知，`job_output` 的 wait 不改这个标记；超时转后台用 `promote` 原子清标记，若已结算未通知则补发一次；结算回调 agent 发 `JobCompleted` 并把完成通知投进会话（忙碌→下一步，空闲→下一轮）；进程退出即消失、不跨重启（照 DSH jobs-local）。
- **唤醒不丢**：轮次收尾检查与 `settle_idle` 的快照之间只有一个显式让出点（`tokio::task::yield_now`），除此之外没有别的 await；且 `wake_driver` 在 Running 时无条件置 `wake_requested`。收尾窗口里到达的 FollowUp/Steer 不会滞留，`idle()` 不会带着待处理输入返回。
- **step 编号**：每轮从 1 开始、在轮内递增；`Phase::Running` 不保存 step，不存在跨 turn 携带（T004 审 K1）。
- **请求层重试**：step 内只对 `EMPTY_RESPONSE` / `RATE_LIMIT` / `SERVER` / `TIMEOUT` / `TRANSPORT` 重试，最多 5 次；延迟 500ms 起指数退避、上限 10s、±10% 抖动（std 派生，不引依赖），服务端 retry-after 优先、超过上限不重试；只对「尚未流出 content delta」的失败重试（避免重复文本），每次重试发 `AgentEvent::Retrying`、不写日志。
- **max-tokens**：输出被截断时 turn/end 如实记 `max-tokens`；写 `assistant/message` 前按 DSH assembler **丢掉全部 `tool-call` 块**（text/reasoning 保留），裁完为空就不写 assistant 消息（与 DSH 写空 content 的差异，handback 记 known-weakness）；被截断的工具调用不执行。sticky 口径照 DSH：后续 completed step 不降级，aborted/error 可覆盖（对 spec 那句话的解释）。
- **推理内容与用量**：assistant 的 reasoning 块（含 signature）进日志并按协议回放成 wire `thinking`；取消产生的 interrupted 消息保留 text/reasoning 前缀但**不带 signature**（照 DSH interruptedBlocks）。用量字段缺项是 `None`，CLI 显示「未知」而不是 0。
- **取消**：流中断时按 DSH `interruptedBlocks` 规则保留已流出的 text/reasoning 前缀（去空白后非空，tool-call 块丢弃）并写成 `interrupted: true` 的 assistant 消息；取消令牌传进 LLM 层，能打断「等响应头」阶段；批执行取消时已开始的调用等结束并按模型顺序提交，未开始的补 `ABORTED_BEFORE_DISPATCH`。取消后会话可以继续：收尾时 abort 置位的轮次返回无 pending、收敛到 `settle_idle`，再由 settle 用新 Phase 重开驱动处理取消后投递的输入（不空转）。
- **续接先修复再接受输入**：`Agent::resume` 先扫描日志，给最后一个没收尾的 turn 补 `tool/result`（已记录 `tool/call` 的调用 → `TOOL_OUTCOME_UNKNOWN`；只有 assistant 调用块、没记录开始的 → `TOOL_NOT_STARTED`）、`step/end`、`turn/end`（用 `Aborted`，不加 DSH 的 `interrupted` 变体），一次批 append 后才起驱动；修复只读日志、不重跑工具，崩溃也不补 `interrupted` assistant 消息（没落盘的流式文本不存在）。
- 一个会话一个 JSONL 文件，路径是 `<data-dir>/sessions/<id>.jsonl`；首行是 header（格式版本、id、cwd、创建时间）。

## T005 接口地基

T005 各票共用这几处接口和约定；字段与签名看代码，这里只记后续票要遵守的部分。

- **AgentConfig**（`crates/agent/src/lib.rs`）：`max_parallel_tool_calls`、`compaction_threshold_ratio`、`compaction_retain_ratio`、`compaction_headroom_tokens`、`spill_max_inline_tokens`、`instruction_roots`、`skill_roots`。默认值照 DSH：10 / 0.8 / 0.16 / 65536 / 12500 / `~/.agents` / `~/.agents/skills`；Agent 还会固定加入 `<data-dir>`（指令根，T005.5）与 `<data-dir>/skills`（skill 根，T005.6）；没有 HOME 时两者为空根，不降级成相对路径（T005.5）（DSH 出处：`agent-loop/src/constants.ts:6`、`compaction-basic/src/config.ts:20,23,75`、`bundle/base/cordis.patch.yml:416`、`skill/skill-filesystem/README.zh.md:54,72`）。`Agent::create` / `resume` 建 agent 时校验一次，非法值返回 `AgentError::InvalidConfig { field, message }`，不等到驱动任务里 panic。T005.0 只做比例式保留（`retainRatio`），不做 DSH 的 `retainTokens` 二选一。
- **AgentEvent**（同上）：已有 `AssistantTextDelta` / `Retrying` / `UsageReported` / `TurnEnded` 不改名、不改字段；T005 新增 `QuestionAsked { questions: Vec<Question> }`（T005.10 引入，T005.12 起携带全部问题）、`CompactionStarted` / `CompactionEnded`（T005.8a/.8b）、`JobCompleted`（T005.4 已实现：后台任务结算时发出，CLI 展示约定在 `handle_event`）。发出方是各票，CLI 展示约定在 `crates/cli/src/main.rs` 的 `handle_event`。
- **会话日志事件与版本**：T005 新增的 `EventKind` 是 `todo/write`（T005.7 已实现：整表快照、derive 不进请求、repair 忽略、回放后写覆盖先写）、`compaction/start` / `compaction/summary` / `compaction/end`（T005.8a 已实现：摘要进事件、derive 用 checkpoint 替换被 shadowed 的 seq 区间、repair 忽略；T005.8b 补 /compact 与一致性）、`compaction/prune`（T005.2 已实现：追加替换视图、原始事件保留，让续接后的派生请求可复现）；其他票不加新类型。这些都是增补事件类型，`FORMAT_VERSION` 保持 1（T005.0 策略）。加类型时在 `crates/session/src/event.rs` 登记，并同步 `derive.rs`（进不进请求）和 `repair.rs`（收尾分类）的 match。`FORMAT_VERSION` 保持 1：新增事件类型是增补，不升；只有改变已提交事件的字段含义或类型、或删除事件时才升。读到不认识的版本直接拒绝，不做就地迁移。
- **ToolContext**（`crates/tools/src/lib.rs`、`crates/tools/src/session.rs`）：`cwd` + `Arc<ToolSessionState>`（`ObservationStore` 文件观察表、`TaskRegistry` 后台任务（T005.4 起实现：`<data-dir>/jobs/session-<id>/`）、`SpillStore`（`<data-dir>/spill/session-<id>/`，T005.2 起可 `save_text`）、`session_id` / `data_dir`（T005.3 给 bash 受管变量用）、`skill_roots`（T005.6 由 Agent 从 AgentConfig 写入，skill 工具用））。`Agent` 在 create/resume 时建一份、每次工具批传入；续接新建空观察表，不继承上一次。工具单元测试用 `ToolContext::new`（空状态、禁用 spill）。
- **shell 受管环境**：`AIOS_*` 是受管命名空间（`AIOS_SHELL=1`、`AIOS_SESSION_ID`、`AIOS_HOME=<data-dir>`，T005.3）；子进程继承的 `AIOS_*` 先清掉再注入（照 DSH `shell-env`）。

## T006 loop 挂点与功能注册表（T006.1a–1e 收口）

T006.1a 接挂点与注册表；1b 起 **compaction 是真实 Capability**；1c 起 **instructions / skill**；1d 起 **todo / spill / reminder**；1d2 起 **ask 是真实 Capability**（`ask_capability.rs`，per-Agent AskState）；1e 把 jobs 结算回调移入 `jobs_capability.rs`（模块服务，不挂 loop 挂点），并把 capability 状态收进 `CapabilityStates`（driver 只装配）。loop 现在只剩 turn/step/输入队列/工具批/重试；prune 是唯一过渡态（driver 每请求调用 + `agent/prune.rs`，T006.4 搬）。

**设计说明（为什么这样接）**

- 编译期固定：`HookPoint` 枚举固定 10 个（strategy §1 六点位 + 工具三阶段拆开 + 压缩触发单列），功能用 `Capability` trait 的固定顺序数组注册（`CapabilityRegistry::production`），没有运行时加载/插件发现（strategy §3、K2）。
- 输入面按 1b–1e 的需要定：`Capability::on_hook` 是异步、返回 `Result<HookOutcome, AgentError>`，参数是 typed `HookEvent<'a>`（每个挂点带该点可用的运行态数据 + `host: &dyn LoopAccess` 访问面）；dyn 兼容用 `HookFuture<'a> = Pin<Box<dyn Future<Output = Result<HookOutcome, AgentError>> + Send + 'a>>`，不新增 async-trait 依赖。
- 访问面 `LoopAccess`：session（`Mutex<SessionStore>`）、events、tools、tool_session（观察表/spill/任务注册表）、model、cwd/data_dir/log_path、instruction_roots/skill_roots、compaction_config、spill_max_inline_tokens，以及 **`append` 写日志入口**（async，内部走 `Inner::append`：落盘后逐事件派发 SessionEvent，不重复派发）；由 driver 的 `Inner` 实现，capabilities 不反向依赖 driver 私有字段/CLI。
- loop 只在固定位置 dispatch，不 import 功能内部类型；1b–1e 搬实现时只换 Capability 实现（必要时按 ARCH 记录扩 HookContext 字段），挂点集合与调用点顺序不变——1a 不再声称「只换实现」是无条件成立。
- 执行顺序与错误：同一挂点按注册顺序执行；首个 Err 短路返回，之前的功能副作用已发生且不回滚；SessionEvent 在日志 append 成功后逐事件派发（日志已提交）；压缩执行与「已确认」通知拆开：`CompactionCapability` 只挂 PreStep/Request/RequestError（不消费 Compaction），三路（PreStep 自动、RequestError 溢出、PreStep trigger=Manual 手动）都经 `compact_if_needed`，其在 should_trigger/select_split 确认后统一派发 `HookEvent::Compaction`（供 prune 等，T006.4）；自动/手动压缩失败不打断本轮/队列，溢出压缩失败时 `compact_if_needed` 返回 Err（摘要失败等）或 Ok(false)（无前缀可压，返回原始 model 错误），第二次溢出直接致命不再派发；Status 在 kick 收敛前派发，错误不阻断收敛（1a no-op，服务接入时再定上报）。
- PreStep 分两阶段（`HookEvent::PreStep { phase }`，不是新挂点）：Inject 在 prune 之前（instructions、skill；按注册表相对顺序），Compact 在 prune 之后（compaction 自动检查）。旧顺序保持为 system/user → Inject（instructions → skill）→ prune → Compact（压缩检查）→ derive；手动 /compact 只走 Compact（trigger=Some(Manual)），instructions/skill 跳过，所以手动压缩不会顺带注入指令/skill。回归断言：`pre_step_phase_order::injection_happens_before_compaction_in_same_turn`（同 turn 压缩时注入先于 compaction/start）与 `manual_compact_no_injection::manual_compact_does_not_inject_instructions_or_skills`。
- 挂点返回值（T006.1d2 四变体）：`HookOutcome { Continue, ToolResultContent(String), ToolResultOverride { index, result }, AbortStep }`；`CapabilityRegistry::dispatch` 返回 `DispatchOutcome { overrides, content, abort }`：按注册顺序收集、同 index 后写胜出、content 取最后非 None、AbortStep 置 abort 优先且**立即停止**（后续能力不再执行）。Content 只允许 ToolResult 阶段，Override 只允许 ToolBefore 阶段（阶段不匹配忽略）；AbortStep 让 run_step 返回 `StepEnd::Aborted`（不是 error）。
- 注册表 per-Agent（T006.1d2）：`production(ask: Arc<AskState>)` 返回 `Vec<Box<dyn Capability>>`；ZST 能力每 Agent Box 一份，顺序不变；AskCapability 持有该 Agent 的 AskState（pending/answers/取消唤醒），`Agent::answer` 继续委托 Inner 再路由到 AskState（签名/CLI 不变）。
- ask 所有权：AskCapability 只写 `question/asked`、`question/cancelled`（走 LoopAccess::append）；answers 的 tool/result 由 driver 在 ToolResult 阶段统一写一条。本票只拦批内第一条 ask 调用（批内后续 ask 仍走占位工具）；answers 形状/多问题语义照 T005.12 不改。
- ToolBefore override：index 对应调用不执行（占位 ask 工具不跑），结果按 calls 顺序对齐后并回；多 index 由 dispatch 聚合（每能力每事件最多一条 override）。
- 依赖方向：`cli → agent → tools → session → model`；agent 内部 `driver → capabilities` 单向。

**挂点表（HookPoint → 派发位置 → typed 数据 → 注册功能）**

| 挂点 | 派发位置（agent/src/driver.rs） | HookEvent 携带 | 注册功能（1a） |
|---|---|---|---|
| PreStep | 两阶段：Inject = 追加本 step 输入消息之后、prune 之前；Compact = prune 之后、derive 之前（手动 /compact：driver 队列 dispatch Compact + trigger=Some(Manual)） | turn/step/host/trigger/phase | Inject：instructions、skill；Compact：compaction（自动检查/手动强制压缩） |
| Request | `model.stream` 之前 | turn/step/host/messages | compaction（token 记账占位） |
| RequestError | `Err(error)` 分支、abort 检查之后（第二次溢出不再派发） | turn/step/host/error | compaction（溢出强制压缩） |
| SessionEvent | `Inner::append` 成功后逐事件 | host/event | instructions（对账；对外推送后续票） |
| ToolBefore | `execute_batch` 之前 | turn/step/host/calls | instructions、skill、ask（ToolResultOverride/AbortStep） |
| ToolResult | 每个 tool/result 落日志前 | turn/step/host/call/result（返回 ToolResultContent；ask 的 answers 也在此由 driver 统一写） | spill |
| ToolAfter | tool/result 落日志后 | turn/step/host/calls/results/cancelled | todo、reminder |
| Compaction | `compaction.rs` 的 `compact_if_needed` 内、should_trigger/select_split 确认之后（自动/溢出/手动三路统一派发） | turn/step/host/trigger | prune（T006.4 迁入）；`CompactionCapability` 不消费此挂点（防自递归） |
| Status | `kick` 收敛到 Idle 之前 | turn/host | jobs（服务推送后续票） |
| TurnStopping | `run_turn` 写 `turn/end` 之前 | turn/host | （保留，服务接入后使用） |

**prune（T006.4 才搬）**：1b 不动 prune 实现与触发（`driver.rs` 每请求调用保留，T005 的 `tool_result_pruned_before_next_request` 语义不变）；注册表里 prune 仍登记在 `Compaction`（no-op 占位）。`compact_if_needed` 现在在确认后统一派发 Compaction（自动/溢出/手动三路共用），T006.4 把 prune 实现搬进能力后三路都会剪；届时删掉 driver 的每请求调用并改该测试期望。

**功能注册表与 1b–1e 访问面（固定顺序；已迁移/待迁移位置）**

| # | 功能 | 挂点 | 从 HookEvent/ho​st 拿什么 | 1a 现实现位置 | 迁移票 |
|---|---|---|---|---|---|
| 1 | compaction | PreStep(Compact)（自动 / 手动 trigger）/ Request / RequestError | messages、host.model/session/compaction_config、error、trigger | **已迁**：`agent/src/compaction_capability.rs` + `agent/src/compaction.rs` | 1b（完成） |
| 2 | prune | Compaction | host.session（日志事件）、trigger | `agent/src/prune.rs`（现路径，driver 每请求调用保留） | T006.4 |
| 3 | instructions | PreStep(Inject)（manual 跳过；SessionEvent/ToolBefore 后续票） | host.instructions_state/session/instruction_roots/data_dir/events | **已迁**：`agent/src/instructions_capability.rs` + `agent/src/instructions.rs` | 1c（完成） |
| 4 | skill | PreStep(Inject)（manual 跳过；ToolBefore 后续票） | host.skill_catalog_state/skill_roots、host.session/events | **已迁**：`agent/src/skill_capability.rs` + `agent/src/skill_catalog.rs` + `tools/src/skills.rs` 的 discover | 1c（完成） |
| 5 | todo | ToolAfter | calls/results、host.session/events | **已迁**：`agent/src/todo_capability.rs` | 1d（完成） |
| 6 | ask | ToolBefore（只拦批内第一条 ask） | calls、host.events/append、AskState、host.current_abort | **已迁**：`agent/src/ask_capability.rs`（AskState per-Agent） | 1d2（完成） |
| 7 | spill | ToolResult | call/result、host.tool_session.spill、host.spill_max_inline_tokens | **已迁**：`agent/src/spill_capability.rs` | 1d（完成） |
| 8 | reminder | ToolAfter | calls/results、host.repeat_chain | **已迁**：`agent/src/reminder_capability.rs` | 1d（完成） |
| 9 | jobs | Status（占位；结算不走挂点） | host.tool_session（任务注册表）、host.events、host.enqueue_context | **已迁**：`agent/src/jobs_capability.rs`（结算回调/通知投递）+ `tools/src/jobs.rs`（注册表） | 1e（完成） |

**修改规则**：挂点集合与注册顺序固定；新增数据在 typed `HookEvent` 上加字段/变体并在本表记录；不得新增运行时加载。 **写日志入口约束**：写日志必须走 `LoopAccess::append`（它内部走 `Inner::append`，落盘后逐事件派发 SessionEvent，不重复派发）；直接锁 `LoopAccess::session` 后 append 不会触发 SessionEvent，1c/1d 禁止这样绕过。

**验收与证据**：1a 证据见 `T006/evidence/T006.1a-hook-grep.log`；1b 见 `T006/evidence/T006.1b-grep.log`；1c 见 `T006/evidence/T006.1c-grep.log`；1d 见 `T006/evidence/T006.1d-grep.log`；1d2 见 `T006/evidence/T006.1d2-grep.log`；1e 见 `T006/evidence/T006.1e-grep.log`（DoD1：loop 除 prune 外无功能代码、jobs 回调移入 jobs_capability、能力状态 CapabilityStates、prune 过渡态记录、206 绿）。注：1a 日志的「PreStep 精确计数 1」是当时快照；1c 起拆分两阶段后 PreStep 构造点为 3 处（自动 Inject/自动 Compact/手动 Compact），HookPoint 集合未变。

## 接缝

- 测试接缝 1：`Agent` 的 6 个入口（create / resume / send / cancel / subscribe / idle）；`Agent::create` / `resume` 的 `AgentConfig` 是公开参数，非法值返回 `AgentError`。
- 测试接缝 2：内置工具的 `Tool::execute`，在临时目录上跑；工具批执行的并行上限、屏障和顺序用可控的测试 Tool（延迟、独占、失败、挂起）测，不靠 sleep。
- 模型 provider 在 HTTP 层替换（真 DeepSeek 适配器指向本地 mock）。