/**
 * Kind 对话的状态与分页（需求 §三.3）：
 * - 打开时只取最后一页（5 轮，sortDirection: 'desc'），照 web/server/bridge.mjs:887 的做法；
 * - 往上滚到顶再取更早的，每页 20 轮，保持当前阅读位置（Virtuoso 的 firstItemIndex）；
 * - 列表里不出现「在数组里找」：进列表前就把每轮整理成 view model，按 id 建索引。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import { answerQuestions, cancelTurn, cardAct, compactThread, connectSession, dismissQuestions, fetchEventsPage, fetchFirstUse, fetchStatus, peekAvailableModels, recheckRoot, reportMonitor, selectSessionModel, sendMessage, setUnauthorizedHandler } from '@/data/service';
import type { AvailableModelRow } from '@/data/service';
import { dayLabelOf } from '@/lib/numbers';
import { useSettingsState } from '@/lib/settingsState';
import { effortName } from './effortName';
import { eventSelectionEffort, serviceSelectionEffort } from './effortSelection';
import type { Frame, RootState, ServiceEvent } from '@/data/service';

/** agent 帧是 Frame 的一个分支（线上帧类型由 Rust 生成）。 */
type AgentFrame = Extract<Frame, { type: 'agent' }>;
import { reconnectDelayMs } from '@/data/reconnect';
import type { GateDetail, GateState } from './KernelGate';
import { contextPercent, contextTokens } from '@/data/turnUsage';
import { getModelSettings } from '@/data/keyring';
import { setHostHome } from '@/data/service';
import { go } from '@/app/navigate';
import type { AskAnswer } from '@/data/service';
import { parseTaskRelay } from '@/data/taskResult';
import { mark } from '@/lib/perf';
import {
  canvasTracks,
  ccViewOf,
  compactionViewOf,
  deriveTurnViews,
  gateVerdictOf,
  mergeCanvasTracks,
  prependOlderViews,
  reconcileTaskCards,
  taskCcFacts,
  upstreamChangedOf,
  userRpcIdOf,
} from './turnViews';
import type { CanvasTrack, TurnLabels, TurnViewData } from './turnViews';
import { applyTaskCardStates } from './taskCardState';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
export type { CompactionView, QuestionView, ToolStep, TurnUserView, TurnViewData } from './turnViews';

export const PAGE_LAST = 5;
export const PAGE_OLDER = 20;
const FIRST_INDEX_BASE = 1_000_000;

function dayKeyOf(seconds: number | null): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  return date.getFullYear() + '-' + date.getMonth() + '-' + date.getDate();
}

/** 一次性算好日期分隔与「上次查看之后」的位置，列表渲染时只查索引，不在数组里找。 */
function decorate(views: TurnViewData[]): {
  dayLabels: Map<string, string>;
  lastSeenId: string | null;
  lastSeenCounts: { moved: number; waiting: number };
} {
  const dayLabels = new Map<string, string>();
  let previous = '';
  for (const view of views) {
    const key = dayKeyOf(view.startedAt);
    if (key !== previous) {
      dayLabels.set(view.id, dayLabelOf(view.startedAt));
      previous = key;
    }
  }
  const lastSeenId = views.length > 4 ? views[views.length - 4].id : null;
  return { dayLabels, lastSeenId, lastSeenCounts: { moved: 3, waiting: 5 } };
}

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

const EMPTY_STATS: SessionStats = {
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

interface KindState {
  views: TurnViewData[];
  byId: Map<string, TurnViewData>;
  firstItemIndex: number;
  nextCursor: string | null;
  /** 当前最老一条日志的 seq（往上翻时 beforeSeq = oldestSeq，不含本条）。 */
  oldestSeq: number | null;
  hasOlder: boolean;
  loadingOlder: boolean;
  initialLoading: boolean;
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
    /**  F2：用户要的档（选择器显示实际档，提示时对比）。 */
    effortActual?: string;
  };
  /** ：用户最后选的档（换到不能调的模型也记着）。 */
  wantedEffort?: string;
  /** ：会话合计（服务 status.stats）。 */
  stats: SessionStats;
  /**  F2：模型信息是否已从服务初始化过（之后 refreshStats 不覆盖用户的选择，避免异步竞态）。 */
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
  /** T041：把 task-read 事实映射进非终态派发卡（Kind 页挂载/轮询用；与任务对话同源）。 */
  setTaskCardStates: (tasks: ReadonlyMap<string, TaskReadHeader>) => void;
  send: (
    text: string,
    options?: {
      pastes?: string[];
      attachments?: { id: string; name: string; bytes: number }[];
      /** ：画布放进来的那一句带着画布身份（id + 标题）。 */
      canvas?: { id: string; title: string };
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
  acceptTaskResult: (callId: string) => Promise<void>;
  /** T053：门结果卡判定（通过/需修正；note 是用户原话，空白当没有）。 */
  judgeGate: (
    callId: string,
    conclusion: 'pass' | 'needs_fix',
    note?: string,
  ) => Promise<void>;
  compact: () => void;
  setModel: (model: KindState['model']) => void;
  /** ：从服务刷新会话合计/汇率/币种。 */
  refreshStats: () => Promise<void>;
  /** ：换思考档（下一轮生效）。 */
  setEffort: (effort: string) => Promise<void>;
  /**  F2：应用一次服务端选择（wanted/actual 都由服务给），补目录字段并提示。 */
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
}

function indexOf(views: TurnViewData[]): Map<string, TurnViewData> {
  const map = new Map<string, TurnViewData>();
  for (const view of views) map.set(view.id, view);
  return map;
}

function applyRootState(root: RootState): void {
  if (root.state === 'ok') {
    const gate = useKindStore.getState().gate;
    if (gate !== 'unreachable' && gate !== 'token') {
      useKindStore.setState({ gate: 'ok', gateDetail: null });
    }
    return;
  }
  const mapped: GateState =
    root.kind === 'lost'
      ? 'lost'
      : root.kind === 'corrupt'
        ? 'broken'
        : root.kind === 'busy'
          ? 'busy'
          : 'unreachable';
  useKindStore.setState({ gate: mapped, gateDetail: { path: root.path ?? undefined, line: root.line ?? undefined } });
}

/** ：当前连接 + 重连状态；lastSeq 是页面已处理到的日志序号（重连补齐去重用）。 */
let activeSocket: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let recheckTimer: ReturnType<typeof setTimeout> | null = null;
let reconnectAttempt = 0;
let lastSeq = 0;
/** 没送到内核的本地消息：viewId -> { requestId, text, pastes, files, canvas }；重发复用同一个 requestId。 */
const unsentSends = new Map<
  string,
  {
    requestId: string;
    text: string;
    pastes: string[];
    files: { id: string; name: string; bytes: number }[];
    canvas?: { id: string; title: string };
  }
>();

function removeView(viewId: string): void {
  useKindStore.setState((state) => {
    const views = state.views.filter((view) => view.id !== viewId);
    return { views, byId: indexOf(views) };
  });
}

/** 发不出去时在对话里留一条「没送到内核」的本地消息（重发成功后移除）。 */
function addUnsentView(
  viewId: string,
  text: string,
  pastes: string[],
  files: { id: string; name: string; bytes: number }[],
  canvas?: { id: string; title: string },
): void {
  useKindStore.setState((state) => {
    if (state.views.some((view) => view.id === viewId)) return {};
    const view: TurnViewData = {
      id: viewId,
      startedAt: null,
      durationMs: null,
      status: 'completed',
      user: { text, elements: [], pastes, files, canvas: canvas ?? null },
      taskResults: [],
      says: [],
      reasoning: null,
      tools: [],
      answer: null,
      thinkingSeconds: 0,
      tokens: { input: 0, output: 0 },
      model: '模型服务',
      cost: null,
      retry: null,
      failure: null,
      question: null,
      compaction: null,
      canvas: null,
      card: null,
      delivery: 'unsent',
      stoppedAt: null,
    };
    const views = state.views.concat(view).sort(
      (left, right) => Number(left.id.split('-')[1]) - Number(right.id.split('-')[1]),
    );
    return { views, byId: indexOf(views) };
  });
}

/** 按已处理到的 lastSeq 重连；失败后指数退避，横幅显示第几次。 */
function openSocket(): void {
  // 同一时刻只留一条连接：recheck → loadInitial 撞上已经连着的旧 socket 时，两条连接
  // 会把同一帧各收一次（实时文字因此重复）。旧连接先关掉，它的 onClose 看到
  // activeSocket 已经换人就直接忽略。
  const previous = activeSocket;
  const socket = connectSession(
    lastSeq,
    (frame) => {
      if (frame.type === 'ready') {
        applyRootState(frame.root);
        const reconnects = reconnectAttempt;
        reconnectAttempt = 0;
        if (reconnects > 0) {
          // 重连成功后把「断过几次」报给服务记监控。
          void reportMonitor('page-reconnect', 'attempts=' + String(reconnects));
        }
        if (recheckTimer !== null) {
          clearTimeout(recheckTimer);
          recheckTimer = null;
        }
        useKindStore.setState({ connection: { state: 'online' }, sessionId: frame.sessionId ?? null });
      }
      applyFrame(frame);
    },
    () => {
      if (activeSocket !== socket) return;
      activeSocket = null;
      reconnectAttempt += 1;
      useKindStore.setState({
        connection: { state: 'reconnecting', attempt: reconnectAttempt },
      });
      // #1：断线 5 秒还没连回来时做一次 recheck，把「服务换了令牌」与「服务连不上」分开
      // recheck 收到 401 时页面进入令牌说明页（页面开着、服务重启后的下一次调用）；
      // 短暂断线由横幅+重连补齐，不打断页面（reconnect.spec）。
      if (recheckTimer === null) {
        recheckTimer = setTimeout(() => {
          recheckTimer = null;
          void useKindStore.getState().recheck();
        }, 5000);
      }
      if (reconnectTimer === null) {
        const delay = reconnectDelayMs(reconnectAttempt);
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          openSocket();
        }, delay);
      }
    },
  );
  activeSocket = socket;
  if (previous && previous !== socket) previous.close();
}

/** 主动断开（离开 Kind 场景）：不触发重连；onClose 看到 activeSocket 已清就忽略。 */
function closeSocket(): void {
  const socket = activeSocket;
  activeSocket = null;
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (recheckTimer !== null) {
    clearTimeout(recheckTimer);
    recheckTimer = null;
  }
  socket?.close();
}

/** /4：本轮已收到的日志事件（首屏页 + live）；实时帧按轮重算 view。 */
let liveEvents: ServiceEvent[] = [];
let contextWindow = 0;
let pendingCompactionAuto = true;
let livePendingCompaction: { id: string; window?: number } | null = null;
let liveTurn: number | null = null;
let liveStream = '';
/**
 * status 只认最新一次请求的回包。
 * 首屏、turn/end、recheck 三处都会刷 status，前一次（旧）的回包可能后到；
 * 每次发起领一个单调序号，回来时不是最新就整份丢掉（stats/cwd/币种/模型都不写），
 * 免得旧数据盖掉新状态。用序号不用 AbortController：被取代的那次也照样发出去了，
 * 服务端要答完，掐断只是把「旧回包」换成「被中断」，判定还得靠序号。
 */
let statusRequest = 0;
/**
 * 选择器的选择版本：applySelection 一变就 +1。首屏 status 可能在用户刚换完模型之后才回来，
 * 那次初始化不能把新选择盖回旧模型（和 statusRequest 同类的旧回包问题）。
 */
let selectionEpoch = 0;
/** ：这一轮正在流的工具参数（callId → 名字 + 拼起来的参数）；只给「正在画」用，不落日志。 */
const liveToolCalls = new Map<string, { name: string; arguments: string }>();

/**
 * T041：Kind 卡片的「未终态任务」状态用 task-read 事实补（和任务对话页头同源）。
 * 只在 Kind 页可见时由 TaskCardStateSync 拉取；终态卡（relay 已给结果状态）不覆盖。
 */
let taskCardStates: ReadonlyMap<string, TaskReadHeader> = new Map();

/** T053：门判定 relay 回填被把关任务卡状态词（live 和刷新共用；按任务 id 记最近一次）。 */
let gateVerdictByTask: ReadonlyMap<string, 'pass' | 'needs_fix'> = new Map();
/** T053：上游变了的下游（upstream_changed relay；交出新 ready 报告时清除）。 */
let upstreamChangedByTask: ReadonlySet<string> = new Set();

function rememberCardSignals(events: readonly ServiceEvent[]): void {
  let verdicts = gateVerdictByTask;
  let changed = upstreamChangedByTask;
  for (const event of events) {
    if (event.type !== 'user/message') continue;
    const message = event.data?.message;
    const routed = gateVerdictOf(message);
    if (routed && verdicts.get(routed.taskId) !== routed.conclusion) {
      verdicts = new Map(verdicts).set(routed.taskId, routed.conclusion);
    }
    const rpcId = userRpcIdOf(message);
    if (rpcId?.startsWith('upstream-changed:')) {
      const downstream = upstreamChangedOf(message);
      if (downstream && !changed.has(downstream.downstreamId)) {
        changed = new Set(changed).add(downstream.downstreamId);
      }
      continue;
    }
    const relay = parseTaskRelay(message, 0);
    if (relay && changed.has(relay.taskId)) {
      const without = new Set(changed);
      without.delete(relay.taskId);
      changed = without;
    }
  }
  gateVerdictByTask = verdicts;
  upstreamChangedByTask = changed;
}

function applyCardSignals(views: TurnViewData[]): TurnViewData[] {
  if (gateVerdictByTask.size === 0 && upstreamChangedByTask.size === 0) return views;
  return views.map((view) => {
    if (
      !view.taskResults.some(
        (fact) => gateVerdictByTask.has(fact.taskId) || upstreamChangedByTask.has(fact.taskId),
      )
    ) {
      return view;
    }
    return {
      ...view,
      taskResults: view.taskResults.map((fact) => {
        const verdict = gateVerdictByTask.get(fact.taskId);
        const upstreamChanged = upstreamChangedByTask.has(fact.taskId);
        if (
          (verdict === undefined || fact.gateVerdict === verdict) &&
          (fact.upstreamChanged === true) === upstreamChanged
        ) {
          return fact;
        }
        return {
          ...fact,
          ...(verdict === undefined ? {} : { gateVerdict: verdict }),
          ...(upstreamChanged ? { upstreamChanged: true } : {}),
        };
      }),
    };
  });
}

/** 折卡之后再套任务库事实（T038 折卡 + T041 状态映射 + T053 门判定/上游变了状态）。 */
function finishViews(views: TurnViewData[]): TurnViewData[] {
  return applyCardSignals(applyTaskCardStates(reconcileTaskCards(views), taskCardStates));
}

/** 从还没流完的参数 JSON 里取 canvas 的 title；还没流到（或半个转义）就先给空串。 */
function canvasTitleOf(partial: string): string {
  const match = /"title"\s*:\s*"((?:[^"\\]|\\.)*)"/.exec(partial);
  if (!match) return '';
  try {
    return JSON.parse('"' + match[1] + '"') as string;
  } catch {
    return '';
  }
}

/** 有确认卡的提议/决定到达：重推所有轮（新卡会把没动过的旧卡标成 replaced）。 */
function refreshCardViews(): void {
  const derived = new Map(deriveTurnViews(liveEvents, turnLabels()).map((view) => [view.id, view]));
  useKindStore.setState((state) => {
    const views = finishViews(state.views.map((view) => derived.get(view.id) ?? view));
    return { views, byId: indexOf(views), ...decorate(views) };
  });
}

/** 一轮的最新日志事件 → 重建这一个 view（其它轮不动）。 */
function refreshTurn(turn: number): void {
  const events = liveEvents.filter(
    (event) =>
      (typeof event.data.turn === 'number' && Number(event.data.turn) === turn) ||
      event.type === 'card/decision',
  );
  const derived = deriveTurnViews(events, turnLabels())[0];
  if (!derived) return;
  const existing = useKindStore.getState().byId.get(derived.id);
  const view =
    derived.compaction || !existing?.compaction
      ? derived
      : { ...derived, compaction: existing.compaction };
  useKindStore.setState((state) => {
    const views = state.views.slice();
    const index = views.findIndex((item) => item.id === view.id);
    if (index >= 0) {
      views[index] = view;
    } else {
      views.push(view);
      views.sort((left, right) => Number(left.id.split('-')[1]) - Number(right.id.split('-')[1]));
    }
    // T038：本轮新到的 relay 卡并回派发卡（派发卡在别的轮里，单轮 derive 看不到它）。
    const reconciled = finishViews(views);
    return {
      views: reconciled,
      byId: indexOf(reconciled),
      ...decorate(reconciled),
      streamingTurnId: view.status === 'inProgress' ? view.id : null,
    };
  });
}

/**
 * 清掉「当前这一段」的实时文字，并把它钉在指定的一轮上。
 * 工具调用/重试开始时，前面流出来的只是这一步的说明，不是回答：清空后回答区不会把它
 * 累计进去，也不会回退显示 view.answer（那一段同样是工具前说明）。turn 为 null 表示
 * 本轮结束，解除钉住，回答区回到日志里的最终 assistant text。
 */
function resetLiveStream(turn: number | null): void {
  liveStream = '';
  useStreamStore.setState({
    turnId: turn === null ? null : 'turn-' + String(turn),
    itemId: null,
    text: '',
  });
}

/** 日志事件 live：追加进本轮事件，重建 view；turn/end 时收掉流式文本。 */
function applyLiveEvent(event: ServiceEvent): void {
  if (event.type === 'keyring/changed') {
    // 设置变化是 seq=0 的合成事件：不进日志、不动 lastSeq；只把屏蔽表重读一遍，
    // 让选择器的「已屏蔽」立刻跟上（不用刷新页面）。
    void useSettingsState.getState().refreshMasks();
    return;
  }
  if (event.seq <= lastSeq) return;
  lastSeq = event.seq;
  rememberCardSignals([event]);
  liveEvents.push(event);
  // T028.5：抄送 Kind 的行（inbox/spliced 或领取后的 user/message）单独成行，不是一轮。
  if (event.type === 'agent/inbox/spliced' || event.type === 'user/message') {
    const facts = taskCcFacts([event]);
    if (facts.length > 0) {
      useKindStore.setState((current) => {
        const views = mergeCcViews(current.views, [event]);
        if (views === current.views) return {};
        return { views, byId: indexOf(views), ...decorate(views) };
      });
    }
  }
  // 日志里的模型选择是权威的：设置页或确认卡在别处改了模型，选择器跟着换。
  if (event.type === 'session/model') {
    const connection = typeof event.data.connection === 'string' ? event.data.connection : '';
    const model = typeof event.data.model === 'string' ? event.data.model : '';
    const wanted =
      typeof event.data.effortWanted === 'string' ? event.data.effortWanted : undefined;
    if (connection && model) {
      const label = useKindStore.getState().modelNames[model] ?? model;
      const effort = eventSelectionEffort(wanted);
      void useKindStore
        .getState()
        .applySelection(
          {
            connectionId: connection,
            modelId: model,
            label,
            effortWanted: effort.wanted,
            effortActual: effort.actual,
          },
          false,
        );
      // 事件不带实际档：实际档由 card-act 回执或下一次 status 补，不在这里猜测；
      // 也不在事件里现发 status——跨连接换模型会重建根 agent，并发 status 会占住会话写锁。
    }
    return;
  }
  // 卡片决定只带来回执：重推所有轮里的确认卡（被新卡替换的旧卡也在别的轮里）。
  if (event.type === 'card/decision') {
    refreshCardViews();
    return;
  }
  if (
    event.type === 'tool/result' &&
    event.data.meta &&
    typeof event.data.meta === 'object' &&
    (event.data.meta as { card?: unknown }).card
  ) {
    refreshCardViews();
  }
  // 画布版本从日志折叠（旧版缩成一行、标题栏切版本、点旧版跳最新都用它）。
  if (event.type === 'tool/result' && event.data.meta) {
    useKindStore.setState((state) => ({
      canvasTracks: mergeCanvasTracks(state.canvasTracks, [event]),
    }));
  }
  // 工具结果回来，这一版已经能显示了；「正在画」那一行收掉（参数增量也不再留）。
  if (event.type === 'tool/result') {
    liveToolCalls.clear();
    useKindStore.setState({ drawingCanvas: null });
  }
  const turn = typeof event.data.turn === 'number' ? Number(event.data.turn) : null;
  if (event.type === 'compaction/start') {
    pendingCompactionAuto = !(event.data.sourceCommandId !== undefined && event.data.sourceCommandId !== null);
    return;
  }
  if (event.type === 'compaction/summary') {
    const auto =
      event.data.sourceCommandId === undefined || event.data.sourceCommandId === null
        ? pendingCompactionAuto
        : false;
    const compaction = compactionViewOf(event.data, auto);
    const window = useKindStore.getState().modelWindows[String(event.data.model ?? '')];
    compaction.before = contextPercent(contextTokens(event.data.usage), window) ?? undefined;
    const compactionId = String(event.data.compactionId ?? String(event.seq));
    const dividerTurn = turn === null ? null : turn + 0.5;
    useKindStore.setState((state) => {
      const id =
        'turn-' + String(dividerTurn ?? state.views.length + 0.5) + '-compaction-' + compactionId;
      const view: TurnViewData = {
        id,
        startedAt: null,
        durationMs: null,
        status: 'completed',
        user: null,
        taskResults: [],
        says: [],
        reasoning: null,
        tools: [],
        answer: null,
        thinkingSeconds: 0,
        tokens: { input: 0, output: 0 },
      model: '模型服务',
        cost: null,
        retry: null,
        failure: null,
        question: null,
        compaction,
        canvas: null,
        card: null,
        delivery: null,
        stoppedAt: null,
      };
      livePendingCompaction = { id, window };
      const exists = state.views.some((item) => item.id === id);
      const views = (exists ? state.views.map((item) => (item.id === id ? view : item)) : state.views.concat(view)).sort(
        (left, right) => Number(left.id.split('-')[1]) - Number(right.id.split('-')[1]),
      );
      return { views, byId: indexOf(views) };
    });
    return;
  }
  if (turn === null) return;
  // 压缩后第一条 assistant/message 算「压缩后」占用。
  if (event.type === 'assistant/message' && livePendingCompaction) {
    const pending = livePendingCompaction;
    livePendingCompaction = null;
    const after = contextPercent(contextTokens(event.data.usage), pending.window) ?? undefined;
    useKindStore.setState((state) => {
      const views = state.views.map((item) => {
        if (item.id !== pending.id || !item.compaction) return item;
        return { ...item, compaction: { ...item.compaction, after } };
      });
      return { views, byId: indexOf(views) };
    });
  }
  if (event.type === 'turn/start') {
    liveTurn = turn;
    resetLiveStream(turn);
  }
  // 工具调用开始：这一步已经流出来的话只是「说明」，不能累计成回答。
  if (event.type === 'tool/call' && turn !== null && turn === liveTurn) {
    resetLiveStream(turn);
  }
  if (event.type === 'turn/end') {
    liveTurn = null;
    liveToolCalls.clear();
    useKindStore.setState({ drawingCanvas: null });
    resetLiveStream(null);
    // 一轮结束后从服务刷新统计（轮/步/token/花费由内核从日志算好）。
    void useKindStore.getState().refreshStats();
  }
  refreshTurn(turn);
}

/** agent 帧 live：流式文本先写进 useStreamStore（TurnRow 的 Answer 只重绘这一条）。 */
function applyAgentEvent(event: AgentFrame['event']): void {
  // canvas 的参数增量到了就先画「正在画 · 标题」；title 可能还没流到，先只画「正在画」。
  // 帧不落日志（日志里只有 assistant stream 的 tool-call-chunks 记录）。
  if (event.type === 'tool-call-delta') {
    if (typeof event.callId !== 'string' || !event.callId) return;
    if (!liveToolCalls.has(event.callId)) {
      // 工具调用开始：这一步已经流出来的文字是「说明」，清掉当前段，别拼进回答。
      resetLiveStream(liveTurn);
    }
    const call = liveToolCalls.get(event.callId) ?? { name: '', arguments: '' };
    if (typeof event.name === 'string' && event.name) call.name = event.name;
    if (typeof event.argumentsDelta === 'string') call.arguments += event.argumentsDelta;
    liveToolCalls.set(event.callId, call);
    if (call.name !== 'canvas' || liveTurn === null) return;
    useKindStore.setState({
      drawingCanvas: { turnId: 'turn-' + String(liveTurn), title: canvasTitleOf(call.arguments) },
    });
    return;
  }
  if (event.type === 'retrying') {
    // 一次尝试失败准备重试：失败那次的半截文字不算回答，重试从空段重新流。
    resetLiveStream(liveTurn);
    return;
  }
  if (event.type === 'usage') {
    const used =
      (event.inputTokens ?? 0) + (event.cacheReadTokens ?? 0) + (event.cacheWriteTokens ?? 0);
    if (contextWindow > 0) {
      useKindStore.setState({ usage: Math.min(99, Math.round((used / contextWindow) * 100)) });
    }
    return;
  }
  if (event.type !== 'text-delta' || typeof event.text !== 'string') return;
  if (liveTurn === null) return;
  liveStream += event.text;
  const id = 'turn-' + String(liveTurn);
  if (!useKindStore.getState().byId.has(id)) {
    refreshTurn(liveTurn);
  }
  useStreamStore.setState({ turnId: id, itemId: null, text: liveStream });
}

function applyFrame(frame: Frame): void {
  if (frame.type === 'ready') {
    // 记住宿主的 home，页面显示路径时缩写成 ~/…。
    setHostHome(frame.host?.home);
    contextWindow = frame.context.window;
    const usage =
      frame.context.window > 0
        ? Math.round((frame.context.used / frame.context.window) * 100)
        : 0;
    useKindStore.setState({ connection: { state: 'online' }, usage });
    return;
  }
  if (frame.type === 'event') {
    applyLiveEvent(frame.event);
    return;
  }
  applyAgentEvent(frame.event);
}

/**
 * T028.5：把抄送 Kind 的行按时间插进列表。
 *
 * 抄送不是一轮：单独一条 view（cc 字段），按发生时间插在第一个更晚的轮之前；
 * 同一条 request id 只成一行（inbox/spliced 与领取后的 user/message 都收）。
 */
function mergeCcViews(views: TurnViewData[], events: ServiceEvent[]): TurnViewData[] {
  const facts = taskCcFacts(events);
  if (facts.length === 0) return views;
  let next = views;
  for (const { cc, at } of facts) {
    const id = 'task-cc-' + cc.inputId;
    if (next.some((view) => view.id === id)) continue;
    const view = ccViewOf(cc, at);
    const index = next.findIndex((item) => item.startedAt !== null && item.startedAt > at);
    next = index < 0 ? next.concat(view) : next.slice(0, index).concat(view, next.slice(index));
  }
  return next;
}

/** 从日志事实派生界面的一轮（打开/历史/往上翻；其余能力逐票补）。 */
const DEMO_MODEL = { connectionId: 'deepseek', modelId: 'deepseek-chat', label: 'DeepSeek Chat' };

/** 派生只拿事件和显示名表；node 用例不装 store 也能跑 turnViews。 */
export function turnLabels(): TurnLabels {
  const state = useKindStore.getState();
  return {
    connectionNames: state.connectionNames,
    modelWindows: state.modelWindows,
    modelNames: state.modelNames,
  };
}

export const useKindStore = create<KindState>()((set, get) => ({
  views: [],
  byId: new Map(),
  firstItemIndex: FIRST_INDEX_BASE,
  nextCursor: null,
  oldestSeq: null,
  hasOlder: false,
  loadingOlder: false,
  initialLoading: true,
  atBottom: true,
  streamingTurnId: null,
  dayLabels: new Map(),
  lastSeenId: null,
  lastSeenCounts: { moved: 0, waiting: 0 },
  firstUse: false,
  gate: 'ok',
  gateDetail: null,
  connection: { state: 'online' },
  sessionId: null,
  cwd: '',
  usage: 0,
  compacting: false,
  modelNames: {},
  modelWindows: {},
  connectionNames: {},
  model: DEMO_MODEL,
  stats: EMPTY_STATS,
  pickerOpen: false,
  modelInitialized: false,
  canvasTracks: {},
  drawingCanvas: null,
  reveal: null,
  revealTurn: (turnId) => set({ reveal: { turnId, at: Date.now() } }),

  bootstrap: async () => {
    try {
      const view = await getModelSettings();
      const names: Record<string, string> = {};
      const windows: Record<string, number> = {};
      const connectionNames: Record<string, string> = {};
      for (const connection of view.connections) {
        for (const model of connection.models) {
          if (model.name) names[model.id] = model.name;
          if (model.contextWindow) windows[model.id] = model.contextWindow;
          connectionNames[model.id] = connection.name;
        }
      }
      for (const provider of view.catalog) {
        for (const model of provider.defaultModels) {
          if (model.name) names[model.id] = model.name;
          if (model.contextWindow) windows[model.id] = model.contextWindow;
        }
      }
      set({
        modelNames: names,
        modelWindows: windows,
        connectionNames,
      });
      useSettingsState.setState({ masked: view.disabledModels });
    } catch {
      // 设置读不到时模型名退回 id、压缩占用不显示（日志里的事实还在）。
    }
    try {
      const first = await fetchFirstUse();
      if (first.firstUse) {
        set({ firstUse: true, initialLoading: false });
        return;
      }
    } catch {
      // 首次使用状态读不到时继续走对话加载；真正原因由 loadInitial 的 gate/横幅呈现。
    }
    await get().loadInitial();
    // 统计/模型信息异步补（不让目录网络请求拖慢页面首屏）。
    void get().refreshStats();
  },

  loadInitial: async () => {
    try {
      const page = await fetchEventsPage({ maxMessages: 100, minMessages: 10, minTurns: 5 });
      rememberCardSignals(page.events);
      const views = finishViews(
        mergeCcViews(deriveTurnViews(page.events, turnLabels()), page.events),
      );
      const lastSeqOfPage = page.events[page.events.length - 1]?.seq ?? 0;
      const oldestSeq = page.events[0]?.seq ?? null;
      liveEvents = page.events.slice();
      liveTurn = null;
      for (const event of page.events) {
        if (event.type === 'turn/start' && typeof event.data.turn === 'number') {
          liveTurn = Number(event.data.turn);
        }
      }
      liveStream = '';
      liveToolCalls.clear();
      useStreamStore.setState({ turnId: null, itemId: null, text: '' });
      lastSeq = lastSeqOfPage;
      openSocket();
      mark('turns:loaded');
      set({
        views,
        byId: indexOf(views),
        ...decorate(views),
        canvasTracks: canvasTracks(page.events),
        drawingCanvas: null,
        firstItemIndex: FIRST_INDEX_BASE - views.length,
        oldestSeq,
        hasOlder: page.hasMore,
        initialLoading: false,
      });
    } catch {
      // 401 已由 service 请求层统一进令牌说明页；这里只标不可达，不覆盖 token。
      const gate: GateState = get().gate === 'token' ? 'token' : 'unreachable';
      set({ initialLoading: false, gate, gateDetail: null });
    }
  },

  loadOlder: async () => {
    const state = get();
    if (!state.hasOlder || state.loadingOlder || state.oldestSeq === null) return;
    set({ loadingOlder: true });
    mark('prepend:start');
    try {
      const page = await fetchEventsPage({
        beforeSeq: state.oldestSeq,
        maxMessages: 400,
        minMessages: 100,
        minTurns: 20,
      });
      const older = mergeCcViews(deriveTurnViews(page.events, turnLabels()), page.events);
      const oldestSeq = page.events[0]?.seq ?? state.oldestSeq;
      set((current) => {
        // 跨页的同一行（抄送的 spliced 与领取后的 user/message）只留已经在列表里的那条。
        // T038：同一任务的派发卡可能在更早的页里，relay 卡要并回去。
        const views = finishViews(prependOlderViews(older, current.views));
        return {
          views,
          byId: indexOf(views),
          ...decorate(views),
          canvasTracks: mergeCanvasTracks(current.canvasTracks, page.events),
          firstItemIndex: current.firstItemIndex - (views.length - current.views.length),
          oldestSeq,
          hasOlder: page.hasMore,
          loadingOlder: false,
        };
      });
      requestAnimationFrame(() => requestAnimationFrame(() => mark('prepend:painted')));
    } catch {
      set({ loadingOlder: false });
    }
  },

  setAtBottom: (value) => set({ atBottom: value }),

  setTaskCardStates: (tasks) => {
    taskCardStates = tasks;
    set((state) => {
      const views = applyTaskCardStates(state.views, tasks);
      if (views === state.views) return {};
      return { views, byId: indexOf(views), ...decorate(views) };
    });
  },

  disconnect: closeSocket,

  send: (text, options) => {
    // 真实发送；requestId 是幂等键（重发复用同一个）。
    // pastes/attachments 跟着发（服务端拼成用户消息的块）。
    // 返回是否受理成功（草稿只在成功时清）。
    const requestId = globalThis.crypto.randomUUID();
    const viewId = 'turn-999-unsent-' + requestId;
    // F1：没送到的那条也把粘贴和文件带着，重发时原样再发一次。
    const pastes = options?.pastes ?? [];
    const files = options?.attachments ?? [];
    const canvas = options?.canvas;
    return sendMessage(text, requestId, options).then(
      () => true,
      () => {
        unsentSends.set(viewId, { requestId, text, pastes, files, canvas });
        addUnsentView(viewId, text, pastes, files, canvas);
        return false;
      },
    );
  },

  resend: (turnId) => {
    const pending = unsentSends.get(turnId);
    if (!pending) return;
    const viewId = turnId;
    void sendMessage(pending.text, pending.requestId, {
      pastes: pending.pastes,
      attachments: pending.files,
      canvas: pending.canvas,
    })
      .then(() => {
        unsentSends.delete(viewId);
        removeView(viewId);
      })
      .catch(() => {
        toast('还是没送到', { description: '连上内核之后再试一次' });
      });
  },

  recheck: async () => {
    try {
      const root = await recheckRoot();
      applyRootState(root);
      if (root.state === 'ok') {
        useKindStore.setState({ gate: 'ok', gateDetail: null });
        await get().loadInitial();
        // 恢复时服务端可能刚接上新连接：把当前模型/档同步到选择器（首次初始化才写）。
        await get().refreshStats();
      }
    } catch {
      // 401 已由 service 请求层统一进令牌说明页；这里只标不可达，不覆盖 token。
      const gate: GateState = get().gate === 'token' ? 'token' : 'unreachable';
      set({ gate, gateDetail: null });
    }
  },

  stop: () => {
    // 真实取消；页面状态由日志 turn/end(aborted) 驱动（不本地猜）。
    void cancelTurn().catch(() => {
      toast('停不下来', { description: '连上内核再试；或等这一轮自己结束' });
    });
  },

  answer: (_turnId, answers) => {
    // 整批真实回答；问题卡状态由 question/answered 事件回填。
    void answerQuestions(
      answers.map((answer) => ({
        questionId: answer.id,
        selected: answer.selected,
        custom: answer.custom,
      })),
    ).catch(() => {
      toast('答案没送到内核', { description: '连上之后再试一次' });
    });
  },

  dismiss: () => {
    // 整批不答；模型收到用户没答后照常收尾。
    void dismissQuestions().catch(() => {
      toast('没送达', { description: '连上之后再试一次' });
    });
  },

  confirmCard: (callId, optionId) => {
    // 卡片状态和回执由 card/decision 事件回填（页面不先改，服务和日志说了算）。
    return cardAct(callId, 'confirm', optionId).then(
      (response) => {
        // 换模型的回执带实际档：卡路径没有 session-model-select 的响应，用这条回填选择器。
        const decision = response.decision;
        const data = decision?.phase === 'confirmed' ? decision.data : undefined;
        const result =
          data && typeof data.result === 'object' && data.result !== null
            ? (data.result as Record<string, unknown>)
            : null;
        if (!result) return;
        const { connection, model, effortWanted, effortActual } = result;
        if (
          typeof connection !== 'string' ||
          typeof model !== 'string' ||
          typeof effortActual !== 'string'
        ) {
          return;
        }
        const wanted = typeof effortWanted === 'string' ? effortWanted : undefined;
        const label = get().modelNames[model] ?? model;
        void get().applySelection(
          {
            connectionId: connection,
            modelId: model,
            label,
            effortWanted: wanted,
            effortActual,
          },
          false,
        );
        if (wanted && wanted !== effortActual) {
          toast(
            '下一轮开始用 ' +
              label +
              ' · 思考 ' +
              effortName(effortActual) +
              '（没有「' +
              effortName(wanted) +
              '」，按实际档）',
          );
        }
      },
      (error: unknown) => {
        toast('确认没送到内核', { description: '连上内核再试一次' });
        throw error;
      },
    );
  },

  dismissCard: (callId) => {
    return cardAct(callId, 'dismiss').then(
      () => {},
      (error: unknown) => {
        toast('暂不没送到内核', { description: '连上内核再试一次' });
        throw error;
      },
    );
  },

  acceptTaskResult: (callId) => {
    // 卡片状态不先改：服务登记后由 card/decision 事件回填「你已接受」。
    return cardAct(callId, 'confirm').then(
      (response) => {
        const decision = response.decision;
        if (decision?.phase === 'confirmed') return;
        const summary =
          decision && typeof decision.data?.summary === 'string'
            ? decision.data.summary
            : '内核没有记下这次接受';
        toast('没接受成', { description: summary });
        throw new Error(summary);
      },
      (error: unknown) => {
        toast('接受没送到内核', { description: '连上内核再试一次' });
        throw error;
      },
    );
  },

  judgeGate: (callId, conclusion, note) => {
    // 判定回执由 card/decision 事件回填；这里只在服务没记下时报原因。
    return cardAct(callId, conclusion === 'pass' ? 'pass' : 'needs-fix', undefined, note).then(
      (response) => {
        const decision = response.decision;
        if (decision?.phase === 'confirmed' || decision?.phase === 'refused') return;
        toast('判定没记下', { description: '内核没有返回判定结果' });
        throw new Error('判定没记下');
      },
      (error: unknown) => {
        toast('判定没送到内核', { description: '连上内核再试一次' });
        throw error;
      },
    );
  },

  compact: () => {
    // 真实手动压缩；分隔线由 compaction/* 日志事件回填（页面不预览假摘要）。
    void compactThread(globalThis.crypto.randomUUID()).catch(() => {
      toast('压缩没成功', { description: '连上内核再试；原文都还在日志里' });
    });
  },

  setModel: (model) => {
    set((state) => ({ model: { ...state.model, ...model } }));
    // 换模型后马上补目录字段（档位/价格/窗口），不等下一轮 refreshStats。
    void get().hydrateModel({
      connectionId: model.connectionId,
      modelId: model.modelId,
      label: model.label,
      effort: model.effort,
    });
    toast('下一轮开始用 ' + model.label);
  },

  applySelection: async (selection, announce = true) => {
    selectionEpoch += 1;
    const { connectionId, modelId, label, effortWanted, effortActual } = selection;
    const wanted = effortWanted ?? get().wantedEffort;
    // 实际档只认调用方给的服务值；没有就是未知，不能拿 wanted 顶替。
    const actual = serviceSelectionEffort(wanted, effortActual).actual;
    set((state) => ({
      wantedEffort: wanted,
      model: {
        ...state.model,
        connectionId,
        modelId,
        label,
        effort: actual ?? state.model.effort,
        effortActual: actual ?? state.model.effortActual,
      },
    }));
    await get().hydrateModel({
      connectionId,
      modelId,
      label,
      effort: actual ?? get().model.effort,
    });
    if (!announce || !actual) return;
    // F2：提示带档；实际档和想要的档对不上时写明「没有」。
    const wantedName = wanted ? effortName(wanted) : null;
    const actualName = actual ? effortName(actual) : null;
    if (wantedName && actualName && wanted !== actual) {
      toast('下一轮开始用 ' + label + ' · 思考 ' + actualName + '（没有「' + wantedName + '」，按「' + actualName + '」）');
    } else if (actualName) {
      toast('下一轮开始用 ' + label + ' · 思考 ' + actualName);
    } else {
      toast('下一轮开始用 ' + label);
    }
  },

  hydrateModel: async (selection) => {
    const { connectionId, modelId, label } = selection;
    let models = catalogCache.get(connectionId);
    if (models === undefined) {
      try {
        models = (await peekAvailableModels(connectionId)).models;
        catalogCache.set(connectionId, models);
      } catch {
        models = [];
      }
    }
    const info = models.find((model) => model.id === modelId);
    set((state) => ({
      model: {
        ...state.model,
        connectionId,
        modelId,
        label: info?.name ?? label,
        price:
          info?.priceInput != null
            ? {
                input: info.priceInput,
                output: info.priceOutput ?? info.priceInput,
                cacheRead: info.priceCacheRead ?? undefined,
                cacheWrite: info.priceCacheWrite ?? undefined,
              }
            : undefined,
        contextWindow: info?.contextWindow ?? undefined,
        thinks: info?.thinks,
        efforts: info?.efforts,
        effort: selection.effort ?? state.model.effort,
        effortActual: selection.effort ?? state.model.effortActual,
      },
    }));
  },

  setPickerOpen: (open) => set({ pickerOpen: open }),

  refreshStats: async () => {
    statusRequest += 1;
    const request = statusRequest;
    const epoch = selectionEpoch;
    try {
      const status = await fetchStatus();
      // 这次已经被后来的请求取代了：整份丢掉，一点旧数据都不写。
      if (request !== statusRequest) return;
      // 工作过程里的路径按会话工作目录相对化；旧服务没给 cwd 时保持空串（照原样显示）。
      set({ cwd: status.cwd ?? '' });
      if (status.stats) {
        const stats = status.stats;
        set({
          stats: {
            turns: stats.turns,
            steps: stats.steps,
            input: stats.inputTokens,
            cacheRead: stats.cacheReadTokens,
            cacheWrite: stats.cacheWriteTokens,
            output: stats.outputTokens,
            cost: stats.costUsd ?? null,
            unpriced: stats.unpriced,
            contextUsed: stats.contextUsed,
            contextWindow: stats.contextWindow,
          },
        });
      }
      // 币种/汇率和设置页共用一个 store，换算只在那里做。
      useSettingsState.getState().applyStatus(status);
      // 当前模型（连接/模型/档由服务 status 给；价格/窗口/档位来自模型目录）：
      // 目录只是补字段，只读地问，不落盘、不动「新」的基准。
      // F2：只在首次初始化时写 model；之后由 applySelection 维护，避免 refreshStats 异步回来覆盖用户刚做的选择。
      if (status.stats && !get().modelInitialized) {
        const stats = status.stats;
        let models = catalogCache.get(stats.connection);
        if (models === undefined) {
          try {
            const catalog = await peekAvailableModels(stats.connection);
            models = catalog.models;
            catalogCache.set(stats.connection, models);
          } catch {
            models = [];
          }
        }
        // 补目录字段期间可能又刷了一次 status；被取代就别再写模型（旧回包不覆盖新状态）。
        if (request !== statusRequest) return;
        // 期间用户已经换过模型（事件/确认卡/选择器）：这次首屏初始化不能再写 model。
        if (selectionEpoch !== epoch) {
          set({ modelInitialized: true });
          return;
        }
        const info = models.find((model) => model.id === stats.model);
        set({
          model: {
            connectionId: stats.connection,
            modelId: stats.model,
            label: info?.name ?? stats.model,
            price:
              info?.priceInput != null
                ? {
                    input: info.priceInput,
                    output: info.priceOutput ?? info.priceInput,
                    cacheRead: info.priceCacheRead ?? undefined,
                    cacheWrite: info.priceCacheWrite ?? undefined,
                  }
                : undefined,
            contextWindow: info?.contextWindow ?? undefined,
            thinks: info?.thinks,
            efforts: info?.efforts,
            effort: stats.effortActual ?? stats.effortWanted,
            effortActual: stats.effortActual,
          },
          wantedEffort: stats.effortWanted,
          modelInitialized: true,
        });
      } else if (status.stats) {
        // 事件换模型只带 wanted：同一 (连接, 模型, wanted) 的 status 回来时补权威实际档。
        // wanted 对不上说明这是换档/换模型之前的旧回包，整块跳过（同口径）。
        const stats = status.stats;
        const current = get().model;
        if (
          stats.connection === current.connectionId &&
          stats.model === current.modelId &&
          stats.effortWanted === get().wantedEffort
        ) {
          set((state) => ({
            model: {
              ...state.model,
              effort: stats.effortActual,
              effortActual: stats.effortActual,
            },
          }));
        }
      }
    } catch {
      // 状态读不到时保留旧值（页面不显示假数据）。
    }
  },

  setEffort: async (effort) => {
    const state = get();
    try {
      const selected = await selectSessionModel({
        connection: state.model.connectionId,
        model: state.model.modelId,
        effortWanted: effort,
      });
      // F2：实际档由服务给（模型没有这一档时是最近档）；换档不弹提示。
      await get().applySelection(
        {
          connectionId: selected.connection,
          modelId: selected.model,
          label: state.model.label,
          effortWanted: selected.effortWanted,
          effortActual: selected.effortActual,
        },
        false,
      );
    } catch {
      toast('思考强度没送达', { description: '连上内核再试一次' });
    }
  },

}));
/** ：连接 id → 模型目录（第一次用到时拉一次；价格/窗口/思考档从这里来）。 */
const catalogCache = new Map<string, AvailableModelRow[]>();

/** 流式文本单独放一个 store：只有最后一条在重绘，别的消息不跟着重建。 */
export const useStreamStore = create<{ turnId: string | null; itemId: string | null; text: string }>(() => ({
  turnId: null,
  itemId: null,
  text: '',
}));

// r4 #1：任何调用拿到 401（service 请求层唯一判断）→ 令牌说明页，先于第一次使用页。
setUnauthorizedHandler(() => {
  useKindStore.setState({
    gate: 'token',
    gateDetail: null,
    firstUse: false,
    initialLoading: false,
  });
  go('kind:conversation');
});
