# AIOS 架构

AIOS 是本地 agent 内核：会话日志是唯一事实来源，loop 只做驱动，功能以能力挂到固定挂点，载体经服务适配。

## 挂点

- **PreStep**：每步之前；Inject 阶段注入指令/skill，Compact 阶段做自动压缩检查。
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

- **model**：provider 无关的消息 IR 与流式块、DeepSeek 适配器、重试/取消、token 估算与请求派生。
- **session**：会话事件与格式版本、JSONL 追加与读取、折叠/恢复、单写者文件锁。
- **tools**：Tool 接口与 ToolSet、工具批执行（独占屏障/并行池/按模型顺序提交）、内置文件工具与观察策略、spill、后台 job、bash。
- **agent**：轮次驱动、收件箱、能力注册表与各能力实现；唯一产品组装入口是 Agent::open。
- **agent 能力**：compaction（自动/溢出/手动）、prune、instructions、skill、todo、ask、spill、reminder、jobs。
- **service**：载体无关的宿主；对象固定 Kind 根会话，调用面 Call/Response，事件面 ServiceEvent。
- **web**：axum HTTP/WS 薄适配器与信任栅栏；ready→事件帧、重连带 lastSeq 补齐。
- **cli**：aios-dev 的 chat/run/serve；只把参数/env 变成普通配置数据，不构造模型/工具/存储。

## 依赖方向

- cli → agent、service、web、model、session、tools
- web → service、agent、model、session
- service → agent、model、session
- agent → tools、session、model
- tools → session、model
- session → model
- model 不依赖其他内部 crate

- 只允许上层依赖下层；下层不反向依赖上层。
- 跨层只走公开接口；axum 只出现在 web，载体无关的服务不依赖 HTTP。
- 测试替身放在公开边界：HTTP 用 wiremock，文件系统用临时目录/注入的 LogWriter。

## 不变量

- **日志只追加**：SessionStore::append 是唯一追加入口；每批先写后 fsync，失败回滚到写前长度；open_read_only 不改文件；尾行不完整只从读视图丢弃。
- **单写者**：create/写 open 在会话锁文件上取非阻塞排他锁；第二个写者立刻得到 AlreadyOwned；只读打开不取锁；锁随句柄/进程释放。
- **每次模型请求从日志派生**：不保留独立消息副本；system/tools/已发消息前缀逐字稳定，供前缀缓存命中。
- **消息身份稳定**：消息 id 在创建时生成，写入日志、续接读回、派生后保持不变。
- **输入队列先落盘再改内存**：send/cancel 返回前队列变更已作为 inbox/spliced 落盘；resume 从日志折叠恢复，不自动开 turn。
- **模型尝试留痕**：已开始流式的尝试写 assistant/message（成功或保留可见前缀的取消）；无可见内容的重试写 assistant/attempt，不进派生历史。
- **工具调用先记日志再派发**：tool/call 在执行前写，tool/result 按模型顺序写；独占工具形成屏障，并行池结果按模型顺序提交。
- **编辑前先读**：文件工具在 seam 调观察策略；未见/过期/缺失分别报 FS_NOT_OBSERVED / FS_STALE_VERSION / FS_NOT_FOUND；观察状态只在会话内。
- **大结果先 spill**：超过阈值的大工具结果整份写进 spill 目录，日志与派生请求只放头尾 preview 与 locator。
- **剪枝只在压缩事务**：只有自动压缩/溢出恢复会剪旧大结果；手动压缩不剪；剪枝写 shadow-price 事件，原事件保留可回放。
- **ask 问题与回答**：问题落 question/asked，整批答完写 question/answered + tool/result；重启后的迟到回答以 user-question-reply 消息接续。
- **指令与 skill 根**：按会话数据目录/项目根/显式 roots 顺序发现；默认不读用户私有目录之外的位置；同名 skill 先出现的根胜出。
- **todo 整表替换**：每次 todo_write 写完整列表与快照；状态三态，校验在工具层。
- **压缩策略**：阈值/保留/预留/重试可配置；摘要请求复用被压前缀；手动压缩运行中返回 busy，不排队。
- **取消与续接**：取消保留可见 text/reasoning 前缀为 interrupted；resume 先修复未收尾的 turn/tool 结果再接受输入。
- **唤醒不丢**：轮次收尾检查与 settle 之间只有一个显式让出点；Running 时 wake 置 latch，输入不会滞留。
- **服务对外只有三类值**：Call/Response 是调用面，ServiceEvent 是事件面；seq/type/data 直接来自日志事实，不另造序号；events 按轮对齐预算。
- **根会话唯一可恢复**：一个用户一条 Kind 根会话；首次原子写状态文件，之后只找回；记录在但日志缺失/状态损坏明确报错，不静默新建。
- **Web 是薄适配器 + 信任栅栏**：只做线上形状，不另做装配/业务；只绑回环地址、每进程一个启动令牌、Host 只认本机；令牌不进日志/证据。
- **CLI/服务入口统一**：写锁冲突映射为 writer-held；停服务先停收新连接，再取消在跑一轮并等收尾，日志完整、锁随 Agent drop 释放。
