//! Kind 会话的 store 状态与统计类型。

import type { AskAnswer, Frame } from '@/data/service';
import type { GateDetail, GateState } from './KernelGate';
import type { CanvasTrack, TurnViewData } from './turnViews';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import type { QueueRestore } from './inbox';

/** agent 帧是 Frame 的一个分支（线上帧类型由 Rust 生成）。 */
export type AgentFrame = Extract<Frame, { type: 'agent' }>;

/** ：会话合计（服务 status.stats 给；页面不自己加）。 */
export interface SessionStats {
  turns: number;
  steps: number;
  input: number;
  cacheRead: number;
  cacheWrite: number;
  output: number;
  cost: number | null;
  unpriced: boolean;
 /** 上下文占用（服务 status.stats 给；页面不自己算）。 */
  contextUsed: number;
  contextWindow: number;
}


export const EMPTY_STATS: SessionStats = {
  turns: 0,
  steps: 0,
  input: 0,
  cacheRead: 0,
  cacheWrite: 0,
  output: 0,
  cost: null,
  unpriced: false,
  contextUsed: 0,
  contextWindow: 0,
};


export interface KindState {
  views: TurnViewData[];
  byId: Map<string, TurnViewData>;
  firstItemIndex: number;
  nextCursor: string | null;
 /** 当前最老一条日志的 seq（往上翻时 beforeSeq = oldestSeq，不含本条）。 */
  oldestSeq: number | null;
  hasOlder: boolean;
  loadingOlder: boolean;
  initialLoading: boolean;
 /** 每次成功加载首屏就 +1（：从别的页点「去看」时等 Kind 页新一屏加载完再滚）。 */
  loadCount: number;
  atBottom: boolean;
  streamingTurnId: string | null;
  dayLabels: Map<string, string>;
  lastSeenId: string | null;
  lastSeenCounts: { moved: number; waiting: number };
 /** ：还没有接模型（空库）；true 时 Kind 首屏是第一次填 key。 */
  firstUse: boolean;
 /** 根会话不可用时的整页说明；ok = 正常对话。 */
  gate: GateState | 'ok';
  gateDetail: GateDetail | null;
  recheck: () => Promise<void>;
 /** 和内核的连接：断了就自动重连，期间不能发。 */
  connection: { state: 'online' } | { state: 'reconnecting'; attempt: number };
 /** ：会话 id（ready 帧给；草稿 localStorage 的键用它）。 */
  sessionId: string | null;
 /** 会话工作目录（status 给；空串=旧服务没给，路径照原样显示）。 */
  cwd: string;
 /** 上下文占用（%），底栏悬停可见。 */
  usage: number;
 /** 上下文窗口 token 数（ready 帧给；0=旧服务没给）。 */
  contextWindow: number;
 /** 上下文已用 token 数（ready 帧给；0=旧服务没给）。 */
  contextUsed: number;
  compacting: boolean;
 /** 演示用：下一条消息送不到。 */
 /** ：模型 id → 显示名（来自模型设置；拿不到就用 id）。 */
  modelNames: Record<string, string>;
 /** ：模型 id → contextWindow（模型设置里的窗口；拿不到就不显示压缩占用）。 */
  modelWindows: Record<string, number>;
 /** ：模型 id → 连接显示名（错误原因里用；取不到写「模型服务」）。 */
  connectionNames: Record<string, string>;
 /** 当前用的模型（底栏的模型选择，； 加价格/窗口/思考档）。 */
  model: {
    connectionId: string;
    modelId: string;
    label: string;
 /** 生效价格（美元/百万 token，服务端按用户价或目录标价算好）；没有价格就是 undefined。 */
    price?: { input: number; output: number; cacheRead?: number; cacheWrite?: number };
    contextWindow?: number;
    thinks?: boolean;
 /** 这个模型能选的档（厂商自己的叫法）；没有就是不能调。 */
    efforts?: string[];
 /** 下一轮实际要发的档（F2：服务端内核算好的实际档；没有想要的档时是最近档）。 */
    effort?: string;
 /** F2：用户要的档（选择器显示实际档，提示时对比）。 */
    effortActual?: string;
  };
 /** ：用户最后选的档（换到不能调的模型也记着）。 */
  wantedEffort?: string;
 /** ：会话合计（服务 status.stats）。 */
  stats: SessionStats;
 /** F2：模型信息是否已从服务初始化过（之后 refreshStats 不覆盖用户的选择，避免异步竞态）。 */
  modelInitialized: boolean;
 /** 模型选择器开着没有（失败块的「换模型」要靠它打开）。 */
  pickerOpen: boolean;
 /** 打开/收起模型选择器。 */
  setPickerOpen: (open: boolean) => void;
 /** ：Kind 首屏引导：先看是不是空库（首次使用），否则加载对话。 */
  bootstrap: () => Promise<void>;
 /** 离开 Kind 场景时主动断开：旧 socket 不在后台收帧（设置页往返不再叠第二条）。 */
  disconnect: () => void;
  loadInitial: () => Promise<void>;
  loadOlder: () => Promise<void>;
  setAtBottom: (value: boolean) => void;
 /** ：把 task-read 事实映射进非终态派发卡（Kind 页挂载/轮询用；与任务对话同源）。 */
  setTaskCardStates: (tasks: ReadonlyMap<string, TaskReadHeader>) => void;
  send: (
    text: string,
    options?: {
      pastes?: string[];
      attachments?: { id: string; name: string; bytes: number; mime?: string | null }[];
 /** ：画布放进来的那一句带着画布身份（id + 标题）。 */
      canvas?: { id: string; title: string };
 /** ：结果卡「要求修改」带进输入框的那张卡。 */
      resultCard?: { taskId: string; title: string; version: number };
 /** （N2）：一次放回多条画布/结果卡，按顺序。 */
      canvases?: { id: string; title: string }[];
      resultCards?: { taskId: string; title: string; version: number }[];
      source?: { path: string; title: string; group: string };
    },
  ) => Promise<boolean>;
  resend: (turnId: string) => void;
  stop: () => void;
  answer: (turnId: string, answers: AskAnswer[]) => void;
  dismiss: (turnId: string) => void;
 /** 确认卡：确认某一项（服务跑核对和执行，回执由 card/decision 事件回填）。 */
  confirmCard: (callId: string, optionId: string) => Promise<void>;
 /** 确认卡：暂不（什么都不执行，记 dismissed）。 */
  dismissCard: (callId: string) => Promise<void>;
 /** 结果卡：接受（服务登记当前版本+报告，回执由 card/decision 事件回填）。 */
  acceptTaskResult: (callId: string, reportId?: string) => Promise<void>;
 /** ：门结果卡判定（通过/需修正；note 是用户原话，空白当没有）。 */
  judgeGate: (
    callId: string,
    conclusion: 'pass' | 'needs_fix',
    note?: string,
  ) => Promise<void>;
  /** 页面从日志折叠出的、被停止清掉的排队消息（Composer 按 seq 最多放回一次）。 */
  queueRestores: QueueRestore[];
  /** 恢复对话后按区间起点把已加载的轮及之后的本地视图去掉（服务端日志区间也藏了）。 */
  dropViewsFromSeq: (startSeq: number, endSeq: number) => void;
  /** 往前翻到日志开头还有被清消息对不上（输入框上方提示，不静默）。 */
  restoreMissing: boolean;
 /** ：点「要求修改」把结果卡放进 Kind 输入框（Composer 消费后清）。 */
  pendingResultCard: { taskId: string; title: string; version: number } | null;
  modifyTaskResult: (card: { taskId: string; title: string; version: number }) => void;
  clearPendingResultCard: () => void;
 /** G1：输入框里现在带着哪张结果卡（发送/删除/清空后清空）；结果卡据此写「已带进输入框」。 */
  composerResultCard: { taskId: string; title: string; version: number } | null;
  setComposerResultCard: (
    card: { taskId: string; title: string; version: number } | null,
  ) => void;
  compact: () => void;
  setModel: (model: KindState['model']) => void;
 /** ：从服务刷新会话合计/汇率/币种。 */
  refreshStats: () => Promise<void>;
 /** ：换思考档（下一轮生效）。 */
  setEffort: (effort: string) => Promise<void>;
 /** F2：应用一次服务端选择（wanted/actual 都由服务给），补目录字段并提示。 */
  applySelection: (
    selection: {
      connectionId: string;
      modelId: string;
      label: string;
      effortWanted?: string;
      effortActual?: string;
    },
    announce?: boolean,
  ) => Promise<void>;
 /** ：选中模型后补目录字段（档位/价格/窗口），让档位选择立刻可用。 */
  hydrateModel: (selection: {
    connectionId: string;
    modelId: string;
    label: string;
    effort?: string;
  }) => Promise<void>;
 /** ：每个画布出过的版本和最新版在哪一轮（从日志折叠）。 */
  canvasTracks: Record<string, CanvasTrack>;
 /** ：正在流式收到的画布标题（工作过程里画「正在画 · 标题」）；没在画是 null。 */
  drawingCanvas: { turnId: string; title: string } | null;
 /** ：点了旧版画布那一行要跳到的那一轮（只影响滚动，不改日志）。 */
  reveal: { turnId: string; at: number } | null;
  revealTurn: (turnId: string) => void;
 /// ：滚到 Kind 对话里的第 turn 轮（已加载直接滚；没加载先往前加载；找不到返回 false）。
  revealTurnNumber: (turn: number) => Promise<boolean>;
 /// ：滚到出这个任务结果卡的那一轮；给了 preferredTurn 时优先选不晚于它的最新一轮
 ///（同一任务多份结果时按 wait 项的轮号取最新卡；没给就选最早）。
  revealTaskResult: (taskId: string, preferredTurn?: number) => Promise<boolean>;
}
