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
- **session**：会话事件与格式版本、JSONL 追加与读取、折叠/恢复、单写者文件锁。
- **tools**：Tool 接口与 ToolSet、工具批执行（独占屏障/并行池/按模型顺序提交）、内置文件工具与观察策略、spill、后台 job、bash。
- **agent**：轮次驱动、收件箱、能力注册表与各能力实现；唯一产品组装入口是 Agent::open。
- **agent 能力**：compaction（自动/溢出/手动）、prune、instructions、skill、todo、ask、spill、reminder、jobs。
- **credentials**：Keyring 真恢复；age 加密整份存储（0600/0700）、age 身份文件作钥匙（换位置/指回/重新开始）、连接描述不含 key 正文、每请求签发并只追加签发记录、变化通知；数据目录配置只存钥匙位置。model::CredentialSource 是模型出站的凭证接缝：每次 stream 前 issue 一次、只用一次；T012.6 起还带 auth_header（连接上记住的单头，None = 两个都发）与 remember_auth_header（适配器学到单头后回报产品层写回连接）；服务验证连接的凭证来源同样写回。
- **service**：载体无关的宿主；对象固定 Kind 根会话，调用面 Call/Response，事件面 ServiceEvent；root 模块负责根会话找回、状态文件与不可用状态；shutdown 做停服务收尾。
- **web**：axum HTTP/WS 薄适配器与信任栅栏；/connect、/ 换签名 cookie，/api、/ws 只认 cookie；同源托管 web/dist 静态资源（SPA fallback）；ready 帧带协议版本、根状态与上下文。
- **web/ 前端**：React 19 + Vite；Kind 对话页只走服务方法与事件；只显示接上内核的部分，不带演示开关/脚本；Kind 首次使用与设置 › 模型与连接接真 Keyring；e2e 用真 serve + 假 DeepSeek 回放。
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
- 测试替身放在公开边界：HTTP 用 wiremock，前端 e2e 用真 serve + 假模型端点，文件系统用临时目录。

## 不变量

- **日志只追加**：SessionStore::append 是唯一追加入口；每批先写后 fsync，失败回滚到写前长度；open_read_only 不改文件；尾行不完整只从读视图丢弃。
- **单写者**：create/写 open 在会话锁文件上取非阻塞排他锁；第二个写者立刻得到 AlreadyOwned；只读打开不取锁；锁随句柄/进程释放。
- **每次模型请求从日志派生**：不保留独立消息副本；system/tools/已发消息前缀逐字稳定，供前缀缓存命中。
- **思考档是模型自己的**：档来自模型目录（models.dev 缓存/快照 + 厂商列表）；要的档与模型档不同时按最近档发（actual≠wanted 时日志两个都记）；压缩摘要用模型的 DefaultEffort。
- **学到的连接事实**：适配器在内存里按「连接 × 模型」学关不掉思考/必须思考（400 后本轮立即按最低非关档重发一次，同进程后续请求直接最低档）；按「连接」学通过的鉴权头（两个头一起被拒后各试一次，x-api-key 先、Authorization 后，学到的单头写回连接的 auth_header，之后只发它）。重启后再学一次；学到的状态不落日志。
- **消息身份稳定**：消息 id 在创建时生成，写入日志、续接读回、派生后保持不变。
- **输入队列先落盘再改内存**：send/cancel 返回前队列变更已作为 inbox/spliced 落盘；resume 从日志折叠恢复，不自动开 turn。
- **模型尝试留痕**：已开始流式的尝试写 assistant/message；无可见内容的重试写 assistant/attempt，不进派生历史。
- **工具调用先记日志再派发**：tool/call 在执行前写，tool/result 按模型顺序写；独占工具形成屏障，并行池结果按模型顺序提交。
- **编辑前先读**：文件工具在 seam 调观察策略；未见/过期/缺失分别报 FS_NOT_OBSERVED / FS_STALE_VERSION / FS_NOT_FOUND；观察状态只在会话内。
- **大结果先 spill**：超过阈值的大工具结果整份写进 spill 目录，日志与派生请求只放头尾 preview 与 locator。
- **剪枝只在压缩事务**：只有自动压缩/溢出恢复会剪旧大结果；手动压缩不剪；剪枝写 shadow-price 事件，原事件保留可回放。
- **ask 问题与回答**：问题落 question/asked，整批答完写 question/answered + tool/result；重启后的迟到回答以 user-question-reply 消息接续。
- **指令与 skill 根**：按会话数据目录/项目根/显式 roots 顺序发现；同名 skill 先出现的根胜出。
- **todo 整表替换**：每次 todo_write 写完整列表与快照；状态三态，校验在工具层。
- **压缩策略**：阈值/保留/预留/重试可配置；摘要请求复用被压前缀；运行中手动压缩返回 busy，不排队。
- **取消与续接**：取消保留可见 text/reasoning 前缀为 interrupted；resume 先修复未收尾的 turn/tool 结果再接受输入。
- **唤醒不丢**：轮次收尾检查与 settle 之间只有一个显式让出点；Running 时 wake 置 latch，输入不会滞留。
- **服务对外只有三类值**：Call/Response 是调用面，ServiceEvent 是事件面；seq/type/data 直接来自日志事实，不另造序号；events 按轮对齐预算。
- **根会话不可用时服务照常起**：ready 帧带 protocolVersion（v2：只增 WS agent 帧 text-delta/usage/retrying，不落日志）、根状态（ok/unavailable kind+path+line+message）和 context（used/window，页面不推算）。
- **Web 是薄适配器 + 信任栅栏**：启动令牌只在 / 与 /connect 换签名 cookie；
  - cookie 绑 hostname+port、HttpOnly/SameSite=Strict/过期；
  - /api、/ws 只认 cookie，Host/Origin 不对 403、未认证 401；
  - sec-fetch-site: cross-site 拒绝；
  - 静态资源只校 Host。
- **协议只增**：根不可用时除 recheck 外方法回 409 ROOT_UNAVAILABLE；recheck 修复后接管；turn/end 的 cause 只增 credential/model。
- **设置面协议 v3**：model-settings、connection-save/delete/verify、keyring-relocate/reopen/restart、issues、first-use、session-model-select 只增；设置由 RootHost 直接读 Keyring（根会话坏了也可读），key 只在 ConnectionSaveRequest.apiKey 请求体，响应/事件不含正文；connection-verify 发最小 Messages 请求并写注册器，session-model-select 覆盖下一轮请求 model（D26），设置变化发 keyring/changed 合成事件；RootHost 用 model_factory 惰性解析 Keyring 连接，空库可起 serve、存连接后 recheck 重建 root agent。
- **CLI/服务入口统一**：写锁冲突映射为 writer-held；停服务先停收新连接，再取消在跑一轮并等收尾，日志完整、锁随 Agent drop 释放。
- **前端只显示能用的部分**：侧栏与路由固定为接上内核的产品状态，不保留演示开关/脚本；不可达页的假数据不进入真路径。
