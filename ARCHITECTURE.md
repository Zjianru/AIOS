# AIOS 架构

AIOS 是本地 agent 内核：会话日志是唯一事实来源，loop 只做驱动，功能以能力挂到固定挂点，载体经服务适配，页面经 Web 薄适配器接服务。

## 挂点

- **PreStep**：每步之前；Inject 注入指令/skill，Compact 做自动压缩检查。
- **Request**：发模型请求时；请求体由会话日志派生，可做 token 记账。
- **RequestError**：请求报错时；上下文溢出走压缩后重试。
- **SessionEvent**：写日志事件后；供对账类功能消费。
- **ToolBefore**：工具执行前；文件观察、指令发现。
- **ToolResult**：工具出结果时；大结果落盘、结果改写。
- **ToolAfter**：工具执行后；落盘收尾与后处理。
- **Compaction**：压缩触发时；剪枝只在压缩事务内发生。
- **Status**：状态变化时；对外推送。
- **TurnStopping**：本轮要停时；收尾推送。

- 挂点顺序在编译期固定；每个能力声明自己关心的挂点，按注册顺序派发。
- 钩子返回 Continue / ToolResult 覆盖 / AbortStep；AbortStep 立即停止后续能力。
- 挂点之外的少量入口只有派发：send/cancel、answer/cancel-question/dismiss、compact、pending/questions、subscribe、idle。

## 功能与模块

- **model**：provider 无关的消息 IR 与流式块、Anthropic Messages 适配器（anthropic/；厂商差异集中在 vendors.rs：思考档按模型目录的档位适配，序列化 thinking/output_config/max_tokens）、重试/取消、token 估算与请求派生、models.dev 目录（24h 缓存/内置快照/厂商列表合并）、本机地址不走代理的共享 HTTP client。
- **session**：会话事件与格式版本、JSONL 追加与读取、折叠/恢复、单写者文件锁；session/model 是会话模型选择（连接/模型/想要的档）的 durable 事实，最近一条胜出（重开恢复）。
- **memory**：记忆库的数据层（SQLite + FTS）：关于你、条目、日记、笔记、修订（只追加）、隔离区、做梦 run 与逐处改动；写入带 actor（User/Kind/Dream）与来源，按 actor 挡越界写入；不依赖 workspace 内其它 crate。服务层的 memory runtime / 做梦运行时、注入与记忆页协议在 service 里装配（见「记忆」一节）。
- **tasks**：任务聚合（Task、Run、Attempt、报告、执行者写入的缺项）与 SQLite 存储（kv/task/effect）；持久效果表（launch/relay/停止 relay/delivery/notice/restart，同键合并、代次、退避、健康）、启动扫描（效果循环前把上次没结束的 Attempt 收成 reported/interrupted/not_opened，打断的排 restart，一次启动合并一条并记下本次打断的任务）、阶段派生（stopped/interrupted → 待补足条件）和席位计数（起跑预约 + 在跑集合，照 DSH WorkerSlots）；不依赖 agent 和 session。
- **calendar**：日历库的数据层与纯规则：约定/版本/运行记录（SQLite，单写者、迁移、schema 版本）、RRULE 规则与注入时钟、复核与合并的纯逻辑；只依赖 rusqlite、serde、rrule、chrono、chrono-tz，不依赖 workspace 内其它 crate。服务层的运行时、送达、循环和页面协议在 service 里装配（见「日历」一节）。
- **tools**：Tool 接口与 ToolSet、工具批执行（独占屏障/并行池/按模型顺序提交）、内置文件工具与观察策略、spill、后台 job、bash、canvas（画布）。
- **agent**：轮次驱动、收件箱、能力注册表与各能力实现；唯一产品组装入口是 Agent::open；Agent::select 写 session/model 并只改下一轮。
- **agent 能力**：compaction（自动/溢出/手动）、prune、instructions、skill、todo、ask、spill、reminder、jobs。
- **credentials**：Keyring 真恢复；age 加密整份存储（0600/0700）、age 身份文件作钥匙（换位置/指回/重新开始）、连接描述不含 key 正文、每请求签发并只追加签发记录、变化通知；数据目录配置只存钥匙位置。model::CredentialSource 是模型出站的凭证接缝：每次 stream 前 issue 一次、只用一次；此后还带 auth_header（连接上记住的单头，None = 两个都发）与 remember_auth_header（适配器学到单头后回报产品层写回连接）；聊天这一路由 CLI 的 KeyringCredentialSource 写回（service 经 model_factory 注入同一个实现）。
- **service**：载体无关的宿主；对象固定 Kind 根会话，调用面 Call/Response，事件面 ServiceEvent；root 模块负责根会话找回、状态文件与不可用状态；shutdown 做停服务收尾；settings 模块管 <数据目录>/settings.json 的非密钥设置（上次模型选择、币种、exec.paused 全局暂停、exec.seats 席位上限）；session-model-select 写事件/上次选择，跨连接空闲时重建根 agent；executor 模块泵 launch/relay/restart 效果、按冻结的任务包开 ThinAgent、把报告和停止事实 Steer 回发起的 Kind 会话、把重启汇报（这次启动打断的任务事实）Steer 进当前 Kind 根并在确认后于原会话开新 Attempt（trigger resume，首条是恢复说明）、立即停（先记 stopped 和停止 relay，再取消 ThinAgent 的轮）、收尾停（插固定停止说明并给会话打标记，除 task_report 外的工具一律拒绝；turn 结束还没报就 waiting 合成后记 stopped）；ThinAgent 报回/交回时若还有后台命令，宿主转 retained：不占席位、不叫醒模型，后台命令结束只落事件，最后一条结算后移除宿主/放锁；页头 backgroundRunning 表示 retained；停止入口（任务页、立即停、Kind task_stop）都能杀 retained 命令；重开同一会话前先停 retained；TaskRuntime::shutdown_all 在退出时先挡新 Attempt、停所有在跑/retained 宿主命令再关 Kind，CLI 的 Ctrl-C/SIGTERM 同一入口，TaskRegistry Drop 对未收 pid 同步 SIGKILL 兜底。ThinAgent 装配 no_wait（命令 sleep、timeout 过 30 秒、长等 job_output 先拒；要等就转后台），一轮结束还有后台命令时保留 Attempt 和席位、落 30 分钟 BackgroundLimit 到效果表，到点杀这一轮命令、把停因 Inject 回会话再合成 waiting，停止/换轮会撤或作废旧到点；取消在这一轮的工具批层按 CancelToken select（丢 future、补 aborted），并 kill_turn 杀这一轮起的进程组；全局暂停只关 launch 准入（在跑的一轮跑完，relay/notice 照送，开闸后按效果表原序接着跑）；launch 再按席位（在跑 + 起跑预约）准入，满了把效果留成 Later 不计失败，改小不掐在跑、只影响之后的准入。任务工具的声明与执行在 service/src/executor/task_tools/（Kind 侧 accept/graph/continue/status/delivery/gate-again/stop/update，ThinAgent 侧 report/handback/ledger note+history，共用参数解析在 params）；门检查运行与解析在 service/src/executor/gating.rs，报告/relay 与回合收尾在 report.rs，账本在 ledger.rs，交付记录在 delivery.rs；executor.rs 只留运行时、效果泵、launch、门/报告落点与装配；工作快照的 graphs 由服务从任务事实算好每条线的 kind/state/program，页面只负责排版和画。
- **web**：axum HTTP/WS 薄适配器与信任栅栏；/connect、/ 换签名 cookie，/api、/ws 只认 cookie；同源托管 web/dist 静态资源（SPA fallback）；/canvas-frame 是画布 iframe 外壳（CSP），/local-file 是本地文件的只读出口（cookie + Host，只放会话工作目录）；ready 帧带协议版本、根状态与上下文。
- **web/ 前端**：React 19 + Vite；Kind 对话页只走服务方法与事件；卡片走注册表 + 信封，槽位按项降级、认不出才退 generic；会话工作目录里的路径显示成相对路径，链接走 /local-file；只显示接上内核的部分，不带演示开关/脚本；Kind 首次使用与设置 › 模型与连接接真 Keyring；e2e 用真 serve + 假 DeepSeek 回放。
- **cli**：aios-dev 的 chat/run/serve；只把参数/env 变成普通配置数据，不构造模型/工具/存储。chat/run 的连接/模型/档优先级是 命令行 > 会话日志选择（resume 恢复）> settings.json 上次选择 > 连接默认；serve 的启动参数（--connection/--model/--effort）只在没有会话/上次选择时当默认，有选择时以选择为准（跨连接热切换不会被启动参数压回）。

## 记忆

- **数据层（memory crate）**：唯一存储是 <数据目录>/memory.sqlite（schema 与迁移在 store）；条目分 about-you / entry / diary / note，note 可由 Kind 或做梦写，about-you 只由用户与采纳写，日记只由压缩前记一笔写；修订只追加，删到隔离区、彻底删留 tombstone 且 FTS 查不到；写入上下文带 actor 与 dream_run，store 按 actor 再挡一次（Kind 写 about-you、Dream 写条目/日记、Dream 改非笔记都拒绝）。
- **注入**：agent 内置一个 PreStep 能力按 provider 取记忆，provider 由服务装配（MemoryRuntime 的 kind/thin provider）；它给出 identity 版本 + 文本（关于你、相关条目），作为一条新来源的 user 消息（MessageSource::MemoryContext）进请求，不复用 instructions 来源、也不改系统指令；identity 变了下一轮追加替换，压缩后再出现，手动压缩不注入。
- **做梦是受限后台 agent**：独立会话（Kind 当前模型快照），只在启动/回合结束/任务报回/记忆页打开/用户说整理几个时机顺手检查一次，不常驻、不另建调度器；忙就等下一个时机，错过多个合成一次。工具只给只读（搜、按 id 读记忆/日记、读 Kind 对话、读任务报回）+ 只写笔记的 memory_dream_apply；读窗口从 DreamSettings 的 last_success_at / last_read_seq 起（工具参数只能往里收），网页/文件/工具输出不读；store 以 actor=Dream 再挡一次。结果写 dream_run + 逐处 dream_change，成功/失败出程序卡；撤销逐处只在当前修订仍等于这次写的版本时成立，用户后来改过就跳过并报告。
- **不进 executor**：记忆与做梦的装配在 service/src/memory/（runtime、工具、页面读写、做梦读任务报回的事实整理），executor 只保留「给任务会话装记忆上下文 provider」这一处注入接线、并只提供任务库的只读读写面（tasks_store / exec_paused），没有记忆业务代码。页面协议见协议 16（MemoryRead/MemoryAction + seq=0 的 memory/changed 推送）。

## 日历

- **数据层（calendar crate）**：唯一存储是 <数据目录>/calendar.sqlite（kv/schema 版本、appointment 当前快照、appointment_version 每版一行只增、run 每次一行只增、watch_state 上次内容）；单写者、Immediate 事务，结果只往前走（排着 → 已送/跳过/没送成，没送成 → 已送）。依赖 rusqlite、serde、rrule 0.14.0、chrono 0.4.45、chrono-tz 0.10.4；不依赖 service/agent/tasks/session/memory。
- **到点怎么进 Kind**：服务 runtime 一条 timer 只睡最近的 next_trigger；到点先写 run（排着），再在 tasks 效果表写 AppointmentWake{appointment_id, run_id}（键是 run id）；效果泵按退避领取，把 User 消息 Steer 进 Kind 根会话（rpc_id appointment-wake:<约定>:<run>，正文 JSON kind=appointment_wake）；根会话不在就留效果等；启动时有 run 没效果行的补上。复核/合并/暂停/循环都是运行时按库与任务事实决定，页面只读 calendar-read、动作走 calendar-action。到点建一条触发任务（origin.role=trigger，只读、不开 Attempt）；Kind 那一轮的 turn/end 钩子给触发任务写 round_end（启动时补写已结束未写的轮），calendar-read 运行块的起止/状态词在读取时从任务事实算。

## 工具、确认卡与根会话

- **条件工具**：工具带 `ToolCondition`（`AfterSkill` / `AfterSkillWithKey`），是否进请求工具表取决于路由能力（T046）：声明了 `addition-only` 的路由（DeepSeek-flash）按条件每步求值一次——本会话成功加载过对应 skill、且宿主已存所需工具凭证，才进这一步的工具表，新增工具连同完整定义记进会话日志（`tool/registry` / `ToolRegistry`），恢复时从日志重建、只加不减，并用 `defer_loading` + `tool_addition` 保持前缀稳定；未声明该能力的路由（MiniMax 等）从第一轮起就把该会话角色 ToolSet 的全部工具声明进请求（条件不再筛工具、skill 只加载内容），整场工具表固定，压缩摘要请求与主请求同一份——角色与安全边界在会话构造期按 `tool_policy.only` 收窄（Kind 不含 write/edit/bash/job_*，ThinAgent 按任务类型 11/13/17 件，含账本两件），此处的全量不放宽边界。未声明路由的全量仍按 key 条件过滤：没填 Exa key 时 web_search/web_fetch 不进表，存 key 后下一轮起才出现（D73）。
- **根会话专属工具**：Kind 的根会话另装 `aios_query`（只读查设置/连接/模型/用量）、`propose_action`（确认卡）、`web_search`/`web_fetch`（要 Exa key 且加载过 `web`）、`task_accept`/`task_status`（派只读调查、查报告）、`task_stop`（立即停 / 收尾停）、`set_paused` 确认卡（全局暂停）和 `resume_task` 确认卡（接续被打断的任务；三者都挂在自带 skill `aios-tasks`，只在这里可见）；CLI 对话不装这些，也不列依赖它们的自带 skill。ThinAgent 的 `task_report` 只在它自己的任务会话里，回执照 launch/relay 效果送到发起会话。
- **任务图（T052）**：`Task` 上的 `parent` / `needs` / `arrangement` 表达「一个总任务 + 若干有先后的子任务」；总任务没有执行承载，不排 launch。图在 `Store::create_graph` 的一个事务里落库。Kind 用 `task_graph_propose` 提议；出卡走 `propose_action` 的 `start_task_graph` 动作（卡上每个子任务一行事实，由动作从任务库读）；用户在卡上确认后 `Store::confirm_graph` 在同一事务里记下确认并把没有 `needs` 的子任务排进 launch。`executor::launch` 统一挡三类：总任务、图没确认、上游还没 ready 的子任务；上游 ready 报告落库时由同一事务放出下游（T052.2）。阶段由 `Store::stage` 派生：子任务图没确认或上游没结果 = 待补足条件，总任务取子任务里按 03 篇 5.1 表序最靠前的阶段。
- **里程碑门（T053）**：图里一个只读子任务带 `Task.gate`（空标记，唯一 `needs` 就是它把关的任务）；被把关任务每次交回新 ready 报告，`Store::record_report` 的同一事务把该报告写进门的 `needs.handoff` 并放门一次（门在跑就等它结束后再放一次，交接当时最新候选）；`Attempt.handoffs` 记开跑时实际拿到的上游报告。判定是内部操作 `gate-judge`（只有用户在门结果卡上点，Kind 没有判定工具）：`Store::judge_gate` 一个事务核对门的最近报告和它那次 Attempt 的候选、去重、写 `GateVerdict`、排 relay；判「通过」时同一事务放出被把关任务的其他下游并交接判通过的那份，`executor::launch` 再统一挡「上游有门但还没判通过」。判需修正后 Kind 用 `task_continue` 让被把关任务在原要求、原写集里接着做（同 Task 新 Attempt、原会话；原因由服务按事实认 GateNeedsFix/UpstreamChanged/AfterReport，任务包里带用户原话和核查发现），新结果报回后门自动重跑；Kind 没有判定工具。
- **程序门（T057）**：图的里程碑子任务带 MilestoneGate.commands（空 = 用户判的 T053 门；有命令 = 程序门，commands_version 改命令 +1）。门仍只读、needs 恰好一个、候选交接/门阶段/接受规则照 T053。executor::launch 在门准入之后按 is_program_gate 分支：不开 ThinAgent、不占席位，begin_attempt_with(GateRecheck) 后 tokio::spawn(run_program_gate)。执行用 tools::jobs::TaskRegistry::detached（root = data-dir/gate-checks/<门>-<Attempt>）+ start_with_spill/wait_async，bash -c、cwd = 门任务目录、环境走新 jobs::command_env()（过滤同 child_env，不带 AIOS_*、不注入 profile）；顺序跑，非 0 继续，超时（默认 120000、上限 600000）/起不来/被停时结束当前进程组并把后面记 Skipped。结论全 0 = Pass、有非 0 = NeedsFix、TimedOut/SpawnFailed/Stopped = Undetermined；逐条 GateCheck 放进 Report.checks（不另开表，SCHEMA_VERSION 仍 2），报告 origin = Program。Store::record_program_gate_result 一个事务写报告+checks+结束 Attempt+判定（judged_by=Program、绑命令版本）+门阶段+放行/relay，判定走与用户判共用的 apply_gate_verdict_in；gate_jobs（task→Attempt/job ids）供立即停结束进程组并记 Stopped/未确定；全局暂停不领新 launch；启动扫描把开着的程序门 Attempt 记 interrupted 并排 restart；resume_task 拒绝程序门，重跑只走 gate_check_again（最新未确定/被打断、候选未变、命令版本未变）；改命令走 update_gate_commands action（整表换+版本+1，门在跑先停记未确定，再对当前候选排 launch）。passed()/needs_fix() 只认 Pass/NeedsFix，且程序门判定要命令版本匹配；Undetermined 阶段派生成 needs_conditions（看板排队·缺条件，不进「等你判定」）。
- **交付记录（T058）**：service::delivery::delivery_for(data_dir, task, gate) 是唯一派生处，门由 tasks 的 Store::guarding_gate 取；锚点=最后一份 state=Ready 且 origin=Agent 的报告（attempt=report.attempt_id.0），之后的 Blocked/Failed/Package 不换锚点、进「还缺什么」，不读 task.missing。五项（交了什么/依据什么/还缺什么/交到了哪/接受了吗）全从既有事实拼，不落库、不加表，schema 仍 2；判定只取 candidate.report_id==锚点的最后一条，程序门命令版本用 Task::verdict_applies（与 passed/needs_fix 共用），未确定原因引用门报告摘要去前缀、没跑命令列 GateCheckOutcome::Skipped；结构化 DeliveryRecord 与 text() 同一函数出。单独只读请求 TaskDelivery{taskId}→{delivery}，不挂 TaskRead：TaskRead 被轮询，changed_files 要读会话日志；页面打开读一次、reportedAt/accepted/门判定变时重读，不加轮询。Kind 工具 task_delivery 读同一份文本；看板 accept 入口「看交付记录」直接进任务页。协议 14→15。
- **上游变了（T053.3）**：上游任务的新可放行报告落库（普通任务的新 ready 报告，或门判通过的新候选）时，`Store::record_report`/`Store::judge_gate` 在同一事务给「已有结果」的下游排一条 `upstream_changed` relay；需重评由事实派生（下游最近 ready 报告那次的 `Attempt.handoffs` 对比上游当前可放行报告 `Store::releasable_report`），不加字段、阶段不回退。Kind 用 `task_continue`（cause UpstreamChanged）让下游按新结果核对，新 ready 报回落库后需重评自然清除；下游结果卡由 relay 派生「上游有新结果，待按新结果核对」。阶段由 `Store::stage` 派生：门报回未判 = 待检查，判了 = 任务要求已满足；被把关任务的最近 ready 被判通过 = 任务要求已满足。
- **接受与已关闭（T054.1）**：`Task.acceptances` 是接受历史（旧行单条 `acceptance` 读成第一项；同报告同版本幂等）；`Store::accept` 一个事务按序核对「报告存在且 ready → 不是门任务 → 是最近报告（已经变了）→ 版本是当前（要求改过了）→ 有门的门已对这份判通过」再追加接受，回执写在 card/decision（`data.summary`「你接受了」+ 时间）；card-act 带卡上的 `reportId`。阶段由 `Store::stage` 派生第六值「已关闭」：当前版本最新 ready 报告被接受（有门的还需门判通过）→ 已关闭；门任务在被把关任务关闭后跟着关闭；总任务全部子任务（含门任务）关闭后关闭。结果卡上的「接受」「要求修改」由前端从会话事实派生（每任务最后一张结果卡、当前版本、未接受、有门已判通过；门任务自己的卡不出），服务端接受时再挡一次。
- **要求修改（T054.2）**：`Task.versions` 每项带要求与 DoD 全文（旧行缺文本，第一次改要求时回填）；`Store::update_requirement` 一个事务先挡「在排队/在跑」与「要求和 DoD 都没变」，再追加第 N+1 版（写集照旧、来源 UserAgreed）并改 Task 的 requirement/dod，旧接受留历史。Kind 用 `propose_action` 的 `update_task_requirement` 动作：describe 出「要求（改前 → 改后，给了 DoD 再加一行）」和「版本（第 N 版 → 第 N+1 版，同一个任务接着做）」两行事实，reconcile 在任务要求已被改过时回「这个任务的要求已经改过了，这张卡是按改之前写的」；确认后同一任务在原会话开新 Attempt（trigger `RequirementChanged`，写集不变）。结果卡上的「要求修改」把卡放进 Kind 输入框：`send` 带 `resultCard`，服务拼成 `result-card` 内容块（模型看到一行「用户要求修改这张结果卡：任务 <id>「标题」，第 N 版要求的结果」，排在用户文字前面），页面画成 240 宽的卡、跟着草稿存；协议升到 11。
- **工作页与「现在」**：看板与「现在」读同一份 Call::Work 快照（WorkResponse{generated_at, tasks, wait}）；service::work 是纯派生：Store::all_tasks() 读全部任务行，工作快照只把「已确认图的子任务 + 单派任务」上板（总任务、未确认图和取消图的子任务都不上；绑总任务的确认卡仍在「等你」），事实含阶段、写集、版本、Attempt 次数、最近报告摘要/问题、接受与关闭时间、after_waiting（有没解析的上游且没有 blocked/failed/提问/被打断，是「等前置」不是问题）；Store::closed_at 按「普通任务看当前版本接受；被把关任务跟着门；总任务没有」算关闭时间；再看 Kind 会话事件派生「等你」——接续、确认卡（未决定/未替换，绑任务归任务桶、未绑的按 call id 各占一项、Kind 提问单独一项互不顶掉）、回答、判门、接受；同一任务命中多种取第一优先、其余进 also，按接续/确认/回答/判门/接受排序。两页共用 web workStore，打开读一次；任务库写入走唯一出口发 task-changed（根 WS 也转发），工作页/日历/任务卡收到才重读，空闲不发请求，workModel 纯派生四列（等你优先）、完成列默认近 7 天（localStorage 只记显示范围）和「现在」三组（等前置不算问题）；「等你」项优先按轮号回 Kind 用 loadOlder 滚到出卡那一轮（kindStore.revealTurnNumber/revealTaskResult，滚动期间不跟底）。

- **组接受（T055.2）**：Store::accept_many 一个事务里逐项走 accept 的同一校验（accept_task），通过的一起记，没通过只回 TaskError 原话；返回逐项 {task_id,title,refused}，同报告同版本幂等；门跟关/总任务汇总照 T054 的派生不改。协议 Call::TaskAcceptMany(TaskAcceptManyRequest{picks}) → TaskAcceptManyResponse{accepted_at,outcomes}（协议仍 12）；网页「等你」勾时存 report_id+version，单项/组接受后立即重取 Work（不等两秒轮询），回执写接受了 N 项/没记 M 项和逐项原因。
- **账本（T056.2）**：账本三样，各有唯一事实源——约束跟着 `RequirementVersion.constraints`（旧行按空；只有 `task_accept`/`task_graph_propose`/`update_task_requirement` 带进来，程序逐字核对发起会话的用户消息原文，对不上整次拒）；叙述条存 `ledger_note` 表（kind 只 rejected/next；同一 kind+topic 新条在同一个事务里关旧条「被新的一条替换」；ready 报告在记录报告的事务里关 next「第 N 次交了结果」；接受报告时在同一事务里关全部「任务已关闭」；`history` 只读全部含关闭原因）；进展/待答不另存，`ledger_for`（service::ledger）从最近报告现编。只有 `ledger_for` 一处编账本段：祖先约束远→近去重标出处、当前任务约束、最近报告 state+summary/question、在用叙述条按类；空项不出，整段超过 6000 字时 `prepare` 记缺项「账本超出上限（N 字，上限 6000）：任务太大，请拆任务」走 blocked，不摘要不截断。注入点：assignment 的 `ledger` 字段、`continue_package` 的三种原因、改要求/改写入范围的说明、`RESUME_NOTE`（只发一次）；压缩成功 `AgentEvent::CompactionEnded{None}` 后执行者用 `ledger-after-compaction:<id>` Steer 当前账本（空账本不发，失败只记日志）；会话日志仍是原始记录，和账本不在同一个事务里（§8 偏差：账本放 SQLite、跟任务事实同事务；日志照旧只追加）。协议 13：TaskReadResponse 增 ledger（constraints/notes/history），TaskReadReport 增 summary/question 供进展与待答显示。 协议 14（T057）：门卡与 TaskRead 增门命令/命令版本/报告 checks/判定 judged_by，TaskReadReport 增 attempt。
- **输入框结果卡同步（T055.3）**：Kind 结果卡上点「要求修改」后，Composer 把输入框里的 resultCard（taskId/标题/版本）同步进 kindStore.composerResultCard；发送、移除或清空时置空，刷新时从 draft localStorage 恢复 resultCard，同一 effect 把状态重建。TaskResultCardView 按 taskId+报告版本匹配时显示「已带进输入框」。同批审计备注：update_task_requirement 的动作 act 先查任务存在/在排队/这一轮在跑/从没开跑，再写要求版本；从没开跑不写库、回「请重新派任务」，顺序与 task_update 一致。
- **取消图（T053.4）**：总任务 `Arrangement.cancelled{at, reason}` 三态——暂不（Dismissed）、被 Kind 新卡替换（Replaced）、Kind 本轮提了图却没上卡（NoCard，下一轮发消息前收尾）；`Store::cancel_graph` 同原因重复取消返回已有记录、已确认的图不能再取消；确认卡动作和发消息入口按卡片事实记取消，`executor::launch` 与 `Store::stage` 统一挡住取消图（子任务不排队、一项不跑），旧确认卡回「已经变了」。
- **确认卡**：模型经 `propose_action` 出卡；选项对应动作登记表（ActionRegistry，负责 details/describe/act）；用户点了才写 `card/decision`（confirmed/refused/failed/dismissed），重开从日志恢复回执；同一张卡的并发/连点只执行一次，回执到之前按钮置灰。

## 依赖方向

- cli → agent、credentials、model、monitor、service、session、tools、web
- web → agent、model、monitor、service、session
- service → agent、calendar、credentials、memory、model、session、tasks、tools
- agent → model、session、tools
- replay → model、session
- monitor → session
- tools → model、session
- session → model
- credentials 不依赖其他内部 crate
- model 不依赖其他内部 crate
- tasks 不依赖其他内部 crate
- memory 不依赖其他内部 crate
- calendar 不依赖其他内部 crate

- 这份清单按 `cargo metadata --no-deps` 的正常 path 依赖（`kind` 为 null）维护；新增或删除内部依赖时同步。
- 只允许上层依赖下层；下层不反向依赖上层。
- 跨层只走公开接口；axum 只出现在 web，载体无关的服务不依赖 HTTP。
- 测试替身放在公开边界：HTTP 用 wiremock，前端 e2e 用真 serve + 假模型端点，文件系统用临时目录。

## 不变量

- **日志只追加**：SessionStore::append 是唯一追加入口；每批先写后 fsync，失败回滚到写前长度；open_read_only 不改文件；尾行不完整只从读视图丢弃。
- **单写者**：create/写 open 在会话锁文件上取非阻塞排他锁；第二个写者立刻得到 AlreadyOwned；只读打开不取锁；锁随句柄/进程释放。
- **每次模型请求从日志派生**：不保留独立消息副本；system/tools/已发消息前缀逐字稳定，供前缀缓存命中。
- **思考档是模型自己的**：档来自模型目录（models.dev 缓存/快照 + 厂商列表）；要的档与模型档不同时按最近档发（actual≠wanted 时日志两个都记）；压缩摘要用模型的 DefaultEffort。
- **会话模型选择**：选择写 session/model（连接/模型/想要的档）是 durable 事实；只从下一轮生效（begin_turn 冻结 model/effort），正在跑的轮不换；重开会话从日志恢复；新会话用 settings.json 的上次选择；跨连接切换在空闲时重建根 agent（运行中只记录选择，不打断本轮）。
- **学到的连接事实**：适配器在内存里按「连接 × 模型」学关不掉思考/必须思考（400 后本轮立即按最低非关档重发一次，同进程后续请求直接最低档）；按「连接」学通过的鉴权头（两个头一起被拒后各试一次，x-api-key 先、Authorization 后，学到的单头写回连接的 auth_header，之后只发它）。重启后再学一次；学到的状态不落日志。
- **消息身份稳定**：消息 id 在创建时生成，写入日志、续接读回、派生后保持不变。
- **输入队列先落盘再改内存**：send/cancel 返回前队列变更已作为 inbox/spliced 落盘；resume 从日志折叠恢复，不自动开 turn。
- **模型尝试留痕**：已开始流式的尝试写 assistant/message；无可见内容的重试写 assistant/attempt，不进派生历史。
- **工具调用先记日志再派发**：tool/call 在执行前写，tool/result 按模型顺序写；独占工具形成屏障，并行池结果按模型顺序提交。
- **编辑前先读**：文件工具在 seam 调观察策略；未见/过期/缺失分别报 FS_NOT_OBSERVED / FS_STALE_VERSION / FS_NOT_FOUND；观察状态只在会话内。
- **大结果先 spill**：超过阈值的大工具结果整份写进 spill 目录，日志与派生请求只放头尾 preview 与 locator。
- **剪枝只在压缩事务**：只有自动压缩/溢出恢复会剪旧大结果和画布旧版（参数/结果里的 html、edits 换成一句，标题与说明保留；最新版不剪）；手动压缩不剪；剪枝写 shadow-price 事件，原事件保留可回放。
- **ask 问题与回答**：问题落 question/asked，整批答完写 question/answered + tool/result；重启后的迟到回答以 user-question-reply 消息接续。
- **指令与 skill 根**：按会话数据目录/项目根/显式 roots 顺序发现；同名 skill 先出现的根胜出。
- **todo 整表替换**：每次 todo_write 写完整列表与快照；状态三态，校验在工具层。
- **压缩策略**：阈值/保留/预留/重试可配置；摘要请求复用被压前缀；运行中手动压缩返回 busy，不排队。
- **取消与续接**：取消保留可见 text/reasoning 前缀为 interrupted；resume 先修复未收尾的 turn/tool 结果再接受输入。
- **唤醒不丢**：轮次收尾检查与 settle 之间只有一个显式让出点；Running 时 wake 置 latch，输入不会滞留。
- **服务对外只有三类值**：Call/Response 是调用面，ServiceEvent 是事件面；seq/type/data 直接来自日志事实，不另造序号；events 按轮对齐预算。
- **根会话不可用时服务照常起**：ready 帧带 protocolVersion（当前 15；屏蔽的模型用 disabled-models-set/check 与 masked 失败原因表达，只增字段/值）、根状态（ok/unavailable kind+path+line+message）和 context（used/window，页面不推算）。
- **Web 是薄适配器 + 信任栅栏**：启动令牌只在 / 与 /connect 换签名 cookie；
  - cookie 绑 hostname+port、HttpOnly/SameSite=Strict/过期；
  - /api、/ws 只认 cookie，Host/Origin 不对 403、未认证 401；
  - sec-fetch-site: cross-site 拒绝；
  - 静态资源只校 Host。
- **协议只增**：根不可用时除 recheck 外方法回 409 ROOT_UNAVAILABLE；recheck 修复后接管；turn/end 的 cause 只增 credential/model。
- **设置面协议 v10**：model-settings、connection-save/delete、keyring-relocate/reopen/restart、issues、first-use、session-model-select 只增；设置由 RootHost 直接读 Keyring（根会话坏了也可读），key 只在 ConnectionSaveRequest.apiKey 请求体，响应/事件不含正文；连接的 check 由保存时拉一次厂商列表写（2xx 且有列表→Ok；401/403→Fail「密钥不对」；其他失败→Fail 原因；2xx 空表保持 Never；拉不到不挡保存），session-model-select 收 connection/model/effortWanted 三个可选字段（只增）并覆盖下一轮请求（D26），设置变化发 keyring/changed 合成事件；RootHost 用 model_factory 按会话选择惰性解析 Keyring 连接（重开传日志里的选择），空库可起 serve、存连接后 recheck 重建 root agent。
- **CLI/服务入口统一**：写锁冲突映射为 writer-held；停服务先停收新连接，再取消在跑一轮并等收尾，日志完整、锁随 Agent drop 释放。
- **前端只显示能用的部分**：侧栏与路由固定为接上内核的产品状态，不保留演示开关/脚本；不可达页的假数据不进入真路径。
- **四类持久文件共用一处原子写**：根状态、设置、汇率缓存、Keyring 私有文件都走 `credentials::atomic`（临时文件写 + fsync 文件 + rename + fsync 目录），失败向上返回；tools/fs 的硬链接原子写另属工具写路径，没有目录 fsync，错误走 ToolFailure。
