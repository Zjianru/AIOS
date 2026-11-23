//! e2e 宿主/假模型场景的选项。

export interface FixtureOptions {
 /// first request 回一次工具调用（read hello.txt），拿到工具结果后再回最终文本。
  toolTurn?: boolean;
 /// ：第一步先流一段「工具前说明」再调 read hello.txt，第二步回最终答案。
 /// 回放 pace 150ms，让「工具之后、最终答案之前」的实时阶段能被 e2e 观察到。
  pretextTurn?: boolean;
 /// ：同上，但 pace 400ms、最终答案 8 段，设置页往返后还有实时流可观察。
  pretextSlowTurn?: boolean;
 /// F5：120 行长流式回答，内容超过视口、流式追加中。
  t059LongStream?: boolean;
 /// 预写根会话日志带三张程序记忆卡（许可/整理完/没跑成），验小卡渲染。
  memoryCards?: boolean;
  /** 记忆替换卡/关于你草稿卡：预置记忆库 + 两轮回放（替换/两条都留、保存/不用）。 */
  t073Cards?: boolean;
 /// 假模型脚本带记忆工具与做梦（记住/记一笔/攒够许可/整理/撤销）。
  memoryFlow?: boolean;
 /// 做梦第一次模型请求强制 400（失败卡原因用例）。
  memoryFailDream?: boolean;
 /// turn3 里 Kind 调 memory_dream_now（许可卡在回合中途到，live 渲染用例）。
  memoryLiveCard?: boolean;
 /// 两轮都记同一句原话（撤销后重新记入用例）。
  memoryAgain?: boolean;
  /// 用真实事件回放工具记忆小卡：一次 memory_remember 成功 + 最终回答。
  t062MemoryCard?: boolean;
 /// 路径显示：三步回放（cwd 内写、cwd 外写、最终文本）；夹具里的 __CWD__ / __OUTSIDE__ 起服务前换成真实目录。
  pathsTurn?: boolean;
 /// ：最终回答里带一个代码块（复制按钮用）。
  codeBlock?: boolean;
 /// 修复：经写任务 ThinAgent 产生 write/edit/bash + task_report 一轮（工作过程页的
 /// 卡片用）。 收窄后 Kind 没有写/改/跑命令，过程只能由 ThinAgent 合法产生。
  t013ProcessTask?: boolean;
 /// ：一轮里 glob + 一个不认识的工具（搜索卡与通用输入/输出卡用）。
  toolCards?: boolean;
 /// ：工具做完后被停下的一轮（工作过程页的「已停止」）。
  t013Interrupted?: boolean;
 /// ：画布夹具。interactive 是拖滑块/换标签、内容超过 420px 的那张；cdn 验地址清单和隔离；
 /// loop 验死循环不卡页面。 加 versions：同一画布两版（第 2 版用 edits 改），再一条第三轮回答。
 /// 加 send：画布里一个「按这个存」按钮（aios.send），第 2 轮的回答另备一条。
 /// 加 streaming：参数拆成多段慢慢流（「正在画 · 标题」）；errors：脚本报错/
 /// 未处理 rejection/CSP 违规三个按钮。
 /// 审计修复加 init-error（画布顶层一打开就 throw）和 forged-export（画布自己发 export）。
 /// 每份都是手写的模型回放脚本（第一轮回 canvas 工具调用，第二轮回一句回答）。
  canvas?:
    | 'interactive'
    | 'cdn'
    | 'loop'
    | 'versions'
    | 'send'
    | 'streaming'
    | 'errors'
    | 'init-error'
    | 'forged-export';
 /// 每次模型回放延迟毫秒（ 停止用例要在运行中按停止/Esc）。
  slowMs?: number;
 /// first request 回一次 ask_user_question（两题：一题选项、一题自己写）。
  askTurn?: boolean;
 /// ：提问卡的降级回放（25 字小标题、空 label 选项、2233 字的问题）。
  questionDegrade?: boolean;
 /// ：让 question/asked 分批到达（毫秒/帧）。
  askTurnPaceMs?: number;
 /// 模型失败脚本：前两次 500 后成功 / 一直 500 / 401 凭证错误。
  failure?: 'retry-then-succeed' | 'model' | 'credential' | 'unknown';
 /// 高上下文占用回放（配小 --context-window），用来触发自动压缩 / 让压缩按钮可用。
  highUsage?: boolean;
 /// serve 的 --context-window（自动压缩测试用 5000）。
  contextWindow?: number;
 /// 在页面和服务之间加一层可断开/恢复的 HTTP+WS 代理（断线重连用例）。
  proxy?: boolean;
 /// 启动前把根会话状态文件做成 lost / corrupt（说明页用例）。
  rootFault?: 'lost' | 'corrupt';
 /// 先起一个 holder serve 占住写锁，测试 serve 起来会报 busy（说明页用例）。
  busyRoot?: boolean;
 /// ：不预置 Keyring 连接（首次使用用例）；serve 仍能起。
  emptyKeyring?: boolean;
 /// ：假厂商 /v1/models 回的模型列表（保存连接会拉一次）。默认 DeepSeek 的两条。
  vendorModels?: string[];
 /// ：启动前把 settings.json 写成坏文件；serve 仍要能起（读设置是宽松的）。
  settingsFault?: 'corrupt';
 /// ：serve 进程的 HOME；传 'dataDir' 用数据目录（页面路径缩写用例）。
  home?: 'dataDir' | string;
 /// ：用带 reasoning/缓存读写的用量夹具（turn-usage 用例）。
  usageTurn?: boolean;
 /// ：缓存命中不到 1%（usage 夹具的 cacheRead=1/input=1000）。
  tinyPercent?: boolean;
 /// F1：用量夹具去掉缓存写（只填缓存读价的端到端用）。
  usageReadOnly?: boolean;
 /// 验收补 B2：回答里裸文本绝对路径 + `open <path>` 代码块（页面渲染成 /local-file 链接）。
  localFilePath?: boolean;
 /// ：用真 key 录的夹具（多步工具/思考/缓存读/压缩）。
  realTurn?: boolean;
 /// ：先加载 skill aios-settings，再查 AIOS 的设置和用量，最后一句回答（回放）。
  t014Query?: boolean;
 /// ：两轮各出一张确认卡（三个选项一张、一个选项一张），回放只给模型流。
  t014Confirm?: boolean;
 /// ：一张只有一个选项的屏蔽卡，回放只给模型流。
  t014Mask?: boolean;
 /// ：先加载 skill web，再收尾一句回答（联网搜索的目录/工具表用）。
  t014Web?: boolean;
 /// ：先加载 skill web，再 web_search 一次，最后回答引用来源 URL（联网搜索回放用）。
  t014Search?: boolean;
 /// ：先加载 skill web，再读一个网页、读一个本机地址，最后收尾（读网页回放用）。
  t014Fetch?: boolean;
 /// ：起一个本地假 Exa；serve 用 --exa-base-url 指过去。
  fakeExa?: boolean;
 /// ：把 EXA_API_KEY 放进 serve 的环境变量，用来证明 key 不从环境变量读。
  envExaKey?: string;
 /// ：起服务前先放一条旧格式（v16）的根会话日志，用来验旧日志照读。
  legacySession?: boolean;
 /// ：把服务的汇率请求指到本地代理，测试可以等 status 进了 fx 的 await 再动作。
  slowFx?: boolean;
 /// ：起服务前先写好根会话日志（翻页夹具）。cc-pages：同一条抄送的
 /// agent/inbox/spliced 落在上一页末尾、领取后的 user/message 落在下一页开头。
  kindHistory?: 'cc-pages';
 /// ：任务结果卡。前几种跑真 Kind + ThinAgent 两个会话（按请求体路由）；
 /// missing / bad-json 用预写好的根会话日志直接验卡与退回文本。
 /// live：同一个任务脚本放慢帧间隔，供「在跑时打开任务对话」的实时用例观察。
  taskResult?:
    | 'ready'
    | 'blocked'
    | 'failed'
    | 'waiting'
    | 'missing'
    | 'bad-json'
    | 'chat'
    | 'live'
 ///.6：任务对话长流式（任务页钉底/滚轮不被拉回的 e2e）。
    | 'long'
 /// ：任务对话输入框/抄送/暂停/Attempt 的夹具。
    | 'say'
    | 'resume'
    | 'handback'
 /// ：两个任务、席位 1，验证叫醒停任务可以多占 1 个。
    | 'seats'
 /// 两个任务在同一轮报回（两张结果卡、刷新同序）。
    | 'two'
 /// ThinAgent 先说话再调工具（任务对话的 say 步）。
    | 'pretext';
 /// ：重启接续夹具。main 四个任务（两个被打断、一个 ready、一个待答），
 /// single 一个被打断的任务；重启那一轮的路由由测试拿到真实任务 id 后再生成。
  t029Restart?: 'main' | 'single';
 /// ：serve 的启动参数（模拟 ds 预检单的 run.sh：`--connection`/`--model` 只当初始默认）。
  startupSelection?: { connection: string; model: string; effort?: string };
 /// ：用 catalog 里不存在的模型 id 起会话，让 route_tool_update 返回 None（测工具表全量固定）。
  routeModel?: string;
 /// ：写任务、接受、bash 开关、工作树、非仓库和改错字的真服务夹具。
 /// worktree 场景会在起服务前把数据目录做成一个 git 仓库；expand 的第二段路由由测试
 /// 拿到真实任务 id 后再生成。
  t030?: 'write' | 'ask' | 'expand' | 'bash' | 'retained' | 'worktree' | 'nonrepo' | 'typo' | 'scope';
  /** T078.2：Kind kind_react + kind_say 分条/表情回放。 */
  t078Say?: boolean;
  /** T078.2：Kind 只回表情、空文本完成。 */
  t078ReactOnly?: boolean;
  /// 监控 WS 双任务夹具（t029 多 task_accept + 放慢 ThinAgent 回放）。
 /// ：任务图夹具——Kind 先 skill 再 task_graph_propose，再 propose_action 出确认卡；
 /// 卡上的 task_id 用 __GRAPH_ID__ 占位，由回放从请求体里换成真图 id。
  t052Graph?: boolean;
 /// ：同一条任务图流程，但第一项 ThinAgent 报 failed（验证下游不自己跑）。
  t052Fail?: boolean;
 /// ：任务图带只读里程碑门（先查 → 独立核查 → 通过后再写）。
  t053Gate?: boolean;
 /// ：调查任务夹具（Kind 直接 task_accept 无写集）；oldCard 是写任务交两份结果；
 /// update 是写任务交结果后改要求（带结果卡进输入框、第 2/3 版要求）。
  t054?: 'investigation' | 'oldCard' | 'update' | 'updateWindow' | 'updateWindow2';
 /// ：工作页读侧夹具。board 是四个 ready 调查任务（等你接受≥4）；
 /// history 是写任务 + 后续多轮文本（较早一轮有结果卡）；confirm 是一张没绑任务的确认卡；
 /// f3 是同一任务两次结果落在不同 Kind 轮（F3 定位敏感性）。
  t055?: 'board' | 'history' | 'confirm' | 'f3';
  /// 任务图夹具：预置三张已确认图（有门等你判 / 两上游汇一下游 / 全部关了）+ 一个单派任务。
  t062TaskGraphs?: boolean;
  /// T064.6：预置 calendar.sqlite 与三条 appointment-wake 的根会话日志，驱动日历页/小条/等你。
  t064Calendar?: boolean;
 /// ：账本夹具。Kind 带约束派出调查任务；ThinAgent 记两条叙述（同 topic 换一条）、
 /// 查一次历史，再交 ready 报告。
  t056?: 'ledger';
 /// ：程序门夹具。Kind 提一张带程序门命令的图；ThinAgent 只交被把关任务的结果，
 /// 门由程序跑命令。
  t057?: 'pass' | 'fail' | 'timeout';
}
