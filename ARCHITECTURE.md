# AIOS 架构

- 本文件只写不常变的东西：代码地图、依赖方向、不变量、接缝。
- 产品与设计文档在 docs/（不提交）；这里不重复。

## 鸟瞰

AIOS 是一个本地 agent 内核：每次模型请求从会话日志派生，工具在本地执行，界面后续接 Tauri。
当前仓库只有第一个内核切片（T004 agent loop）的骨架。

## 代码地图

- crates/model：provider 无关的消息 IR（Message / ContentBlock / Usage / FinishReason / StreamChunk；assistant 的 reasoning 块带 signature），DeepSeek 的 Messages 适配器（请求序列化、SSE 解码、流事件翻译、错误码、retry-after），以及具体类型 `CancelToken`（Arc<AtomicBool>+Notify，供 LLM 层打断请求）。
- crates/session：会话事件（turn/step/消息 + `compaction/prune` 的修剪视图事件），只追加的 JSONL 存储（SessionStore），从日志派生请求消息（应用修剪替换视图），以及续接时的中断修复（`repair`：崩溃/被杀后给没收尾的 turn 补收尾事件）。
- crates/tools：Tool 接口、ToolCall/ToolResult、ToolSet、工具批执行（独占屏障、有界并行池、按模型顺序提交），内置工具：只读 read / glob / grep（进程内搜索，不起 rg 子进程）和写类 write / edit / apply_patch（整文件写、字面替换、Codex 风格补丁；文件系统语义在 `crates/tools/src/fs`），大工具结果的 spill 存储与保留（`spill`：按会话目录落盘、头/尾 preview + locator），通用 bash 工具（每次新 `bash -c`、超时转后台、输出尾部截断 + 全文 spill 路径、受管 `AIOS_*` 环境变量），后台任务三件 job_output / job_list / job_kill 与按会话隔离的任务注册表（`jobs`：子进程流式收集、超时转后台、结算回调），以及工具用的会话上下文 `ToolContext`（cwd + 观察表 / 任务注册表 / spill，见「T005 接口地基」）。
- crates/agent：一轮多步的驱动、收件箱（follow-up / steer / inject）、工具调用的派发与结果回写、Agent 的 6 个驱动入口（create / resume / send / cancel / subscribe / idle）、公开配置 `AgentConfig`、订阅者事件 `AgentEvent`、后台任务完成投递（结算时发 `JobCompleted` 并把完成通知投进下一步输入）；另有 id / log_path 两个只读元数据访问器，不属于驱动入口。集成测试的 mock 响应器、可控测试工具和公共 setup 在 `crates/agent/tests/common`。
- crates/cli：开发用 CLI `aios-dev`（`chat` / `run`）。

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
- **bash 每次新进程**：每次调用起新的 `bash -c`（不持久化）；stdout/stderr 超过 64,000 字节只留尾部、完整流写进会话 spill 目录并在结果里给路径；非零退出是结果（`[exit code: N]` marker），不是错误；超时未结束的命令留在后台当 job（返回 job id），不杀进程；父环境的 KEY/PASSWORD/SECRET/TOKEN（大小写不敏感）与 DSH_*/AIOS_* 变量不传给子进程、再注入受管 AIOS_*（挡凭据泄漏，照 DSH scrubbedParentEnv）；不做沙箱（D38），工具描述不宣称受限（T005.3）。
- **后台任务按会话隔离、跨 turn 存续**：任务注册表挂在 `ToolSessionState`，id 为 `bash-<pid>-<nanos>-<n>`；输出由读线程流式落到 `<data-dir>/jobs/session-<id>/`（目录 0700 / 文件 0600，每 job truncate 新建）并在内存保留尾部；前台 bash 在 `start(..., foreground)` 时就标记 awaited，结算时若已被前台等住就不再发通知，`job_output` 的 wait 不改这个标记；超时转后台用 `promote` 原子清标记，若已结算未通知则补发一次；结算回调 agent 发 `JobCompleted` 并把完成通知投进会话（忙碌→下一步，空闲→下一轮）；进程退出即消失、不跨重启（照 DSH jobs-local）。
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

- **AgentConfig**（`crates/agent/src/lib.rs`）：`max_parallel_tool_calls`、`compaction_threshold_ratio`、`compaction_retain_ratio`、`compaction_headroom_tokens`、`spill_max_inline_tokens`、`instruction_roots`、`skill_roots`。默认值照 DSH：10 / 0.8 / 0.16 / 65536 / 12500 / `~/.agents` / `~/.agents/skills`（DSH 出处：`agent-loop/src/constants.ts:6`、`compaction-basic/src/config.ts:20,23,75`、`bundle/base/cordis.patch.yml:416`、`skill/skill-filesystem/README.zh.md:54,72`）。`Agent::create` / `resume` 建 agent 时校验一次，非法值返回 `AgentError::InvalidConfig { field, message }`，不等到驱动任务里 panic。T005.0 只做比例式保留（`retainRatio`），不做 DSH 的 `retainTokens` 二选一。
- **AgentEvent**（同上）：已有 `AssistantTextDelta` / `Retrying` / `UsageReported` / `TurnEnded` 不改名、不改字段；T005 新增 `QuestionAsked`（T005.10）、`CompactionStarted` / `CompactionEnded`（T005.8a/.8b）、`JobCompleted`（T005.4 已实现：后台任务结算时发出，CLI 展示约定在 `handle_event`）。发出方是各票，CLI 展示约定在 `crates/cli/src/main.rs` 的 `handle_event`。
- **会话日志事件与版本**：T005 计划新增的 `EventKind` 是 `todo/write`（T005.7）、`compaction/start` / `compaction/summary` / `compaction/end`（T005.8a/.8b）、`compaction/prune`（T005.2 已实现：追加替换视图、原始事件保留，让续接后的派生请求可复现）；其他票不加新类型。加类型时在 `crates/session/src/event.rs` 登记，并同步 `derive.rs`（进不进请求）和 `repair.rs`（收尾分类）的 match。`FORMAT_VERSION` 保持 1：新增事件类型是增补，不升；只有改变已提交事件的字段含义或类型、或删除事件时才升。读到不认识的版本直接拒绝，不做就地迁移。
- **ToolContext**（`crates/tools/src/lib.rs`、`crates/tools/src/session.rs`）：`cwd` + `Arc<ToolSessionState>`（`ObservationStore` 文件观察表、`TaskRegistry` 后台任务（T005.4 起实现：`<data-dir>/jobs/session-<id>/`）、`SpillStore`（`<data-dir>/spill/session-<id>/`，T005.2 起可 `save_text`）、`session_id` / `data_dir`（T005.3 给 bash 受管变量用））。`Agent` 在 create/resume 时建一份、每次工具批传入；续接新建空观察表，不继承上一次。工具单元测试用 `ToolContext::new`（空状态、禁用 spill）。
- **shell 受管环境**：`AIOS_*` 是受管命名空间（`AIOS_SHELL=1`、`AIOS_SESSION_ID`、`AIOS_HOME=<data-dir>`，T005.3）；子进程继承的 `AIOS_*` 先清掉再注入（照 DSH `shell-env`）。

## 接缝

- 测试接缝 1：`Agent` 的 6 个入口（create / resume / send / cancel / subscribe / idle）；`Agent::create` / `resume` 的 `AgentConfig` 是公开参数，非法值返回 `AgentError`。
- 测试接缝 2：内置工具的 `Tool::execute`，在临时目录上跑；工具批执行的并行上限、屏障和顺序用可控的测试 Tool（延迟、独占、失败、挂起）测，不靠 sleep。
- 模型 provider 在 HTTP 层替换（真 DeepSeek 适配器指向本地 mock）。