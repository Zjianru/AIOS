# AIOS 架构

- 本文件只写不常变的东西：代码地图、依赖方向、不变量、接缝。
- 产品与设计文档在 docs/（不提交）；这里不重复。

## 鸟瞰

AIOS 是一个本地 agent 内核：每次模型请求从会话日志派生，工具在本地执行，界面后续接 Tauri。
当前仓库只有第一个内核切片（T004 agent loop）的骨架。

## 代码地图

- crates/model：provider 无关的消息 IR（Message / ContentBlock / Usage / FinishReason / StreamChunk；assistant 的 reasoning 块带 signature），DeepSeek 的 Messages 适配器（请求序列化、SSE 解码、流事件翻译、错误码、retry-after），以及具体类型 `CancelToken`（Arc<AtomicBool>+Notify，供 LLM 层打断请求）。
- crates/session：会话事件（turn/step/消息），只追加的 JSONL 存储（SessionStore），从日志派生请求消息，以及续接时的中断修复（`repair`：崩溃/被杀后给没收尾的 turn 补收尾事件）。
- crates/tools：Tool 接口、ToolCall/ToolResult、ToolSet、工具批执行（独占屏障、有界并行池、按模型顺序提交），以及内置只读工具 read / glob / grep（进程内搜索，不起 rg 子进程）。
- crates/agent：一轮多步的驱动、收件箱（follow-up / steer / inject）、工具调用的派发与结果回写、Agent 的 6 个驱动入口（create / resume / send / cancel / subscribe / idle）、公开配置 `AgentConfig`（`max_parallel_tool_calls`，默认 10，必须 ≥1）；另有 id / log_path 两个只读元数据访问器，不属于驱动入口。
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
- **唤醒不丢**：轮次收尾检查与 `settle_idle` 的快照之间只有一个显式让出点（`tokio::task::yield_now`），除此之外没有别的 await；且 `wake_driver` 在 Running 时无条件置 `wake_requested`。收尾窗口里到达的 FollowUp/Steer 不会滞留，`idle()` 不会带着待处理输入返回。
- **step 编号**：每轮从 1 开始、在轮内递增；`Phase::Running` 不保存 step，不存在跨 turn 携带（T004 审 K1）。\n- **请求层重试**：step 内只对 `EMPTY_RESPONSE` / `RATE_LIMIT` / `SERVER` / `TIMEOUT` / `TRANSPORT` 重试，最多 5 次；延迟 500ms 起指数退避、上限 10s、±10% 抖动（std 派生，不引依赖），服务端 retry-after 优先、超过上限不重试；只对「尚未流出 content delta」的失败重试（避免重复文本），每次重试发 `AgentEvent::Retrying`、不写日志。
- **max-tokens**：输出被截断时 turn/end 如实记 `max-tokens`；写 `assistant/message` 前按 DSH assembler **丢掉全部 `tool-call` 块**（text/reasoning 保留），裁完为空就不写 assistant 消息（与 DSH 写空 content 的差异，handback 记 known-weakness）；被截断的工具调用不执行。sticky 口径照 DSH：后续 completed step 不降级，aborted/error 可覆盖（对 spec 那句话的解释）。
- **推理内容与用量**：assistant 的 reasoning 块（含 signature）进日志并按协议回放成 wire `thinking`；取消产生的 interrupted 消息保留 text/reasoning 前缀但**不带 signature**（照 DSH interruptedBlocks）。用量字段缺项是 `None`，CLI 显示「未知」而不是 0。
- **取消**：流中断时按 DSH `interruptedBlocks` 规则保留已流出的 text/reasoning 前缀（去空白后非空，tool-call 块丢弃）并写成 `interrupted: true` 的 assistant 消息；取消令牌传进 LLM 层，能打断「等响应头」阶段；批执行取消时已开始的调用等结束并按模型顺序提交，未开始的补 `ABORTED_BEFORE_DISPATCH`。取消后会话可以继续：收尾时 abort 置位的轮次返回无 pending、收敛到 `settle_idle`，再由 settle 用新 Phase 重开驱动处理取消后投递的输入（不空转）。
- **续接先修复再接受输入**：`Agent::resume` 先扫描日志，给最后一个没收尾的 turn 补 `tool/result`（已记录 `tool/call` 的调用 → `TOOL_OUTCOME_UNKNOWN`；只有 assistant 调用块、没记录开始的 → `TOOL_NOT_STARTED`）、`step/end`、`turn/end`（用 `Aborted`，不加 DSH 的 `interrupted` 变体），一次批 append 后才起驱动；修复只读日志、不重跑工具，崩溃也不补 `interrupted` assistant 消息（没落盘的流式文本不存在）。
- 一个会话一个 JSONL 文件，路径是 `<data-dir>/sessions/<id>.jsonl`；首行是 header（格式版本、id、cwd、创建时间）。

## 接缝

- 测试接缝 1：`Agent` 的 6 个入口（create / resume / send / cancel / subscribe / idle）；`Agent::create` / `resume` 的 `AgentConfig` 是公开参数，非法值返回 `AgentError`。
- 测试接缝 2：内置工具的 `Tool::execute`，在临时目录上跑；工具批执行的并行上限、屏障和顺序用可控的测试 Tool（延迟、独占、失败、挂起）测，不靠 sleep。
- 模型 provider 在 HTTP 层替换（真 DeepSeek 适配器指向本地 mock）。
