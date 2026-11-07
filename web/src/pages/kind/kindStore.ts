/**
 * Kind 对话的状态与分页（需求 §三.3）：
 * - 打开时只取最后一页（5 轮，sortDirection: 'desc'），照 web/server/bridge.mjs:887 的做法；
 * - 往上滚到顶再取更早的，每页 20 轮，保持当前阅读位置（Virtuoso 的 firstItemIndex）；
 * - 列表里不出现「在数组里找」：进列表前就把每轮整理成 view model，按 id 建索引。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import { answerQuestions, cancelTurn, compactThread, connectSession, dismissQuestions, fetchAvailableModels, fetchEventsPage, fetchFirstUse, fetchStatus, recheckRoot, reportMonitor, selectSessionModel, sendMessage, setUnauthorizedHandler, updateSettings } from '@/data/service';
import type { ServiceCatalogModel } from '@/data/service';
import type { Currency } from '@/lib/numbers';
import { effortName } from './effortName';
import type { AgentFrame, Frame, ServiceEvent } from '@/data/service';
import { reconnectDelayMs } from '@/data/reconnect';
import type { GateDetail, GateState } from './KernelGate';
import { contextPercent, contextTokens } from '@/data/turnUsage';
import { getModelSettings } from '@/data/keyring';
import { setHostHome } from '@/data/service';
import { go } from '@/app/navigate';
import type { AskAnswer } from '@/data/fixtures/kindLive';
import { mark } from '@/lib/perf';
import { canvasTracks, compactionViewOf, deriveTurnViews, mergeCanvasTracks } from './turnViews';
import type { CanvasTrack, TurnViewData } from './turnViews';
export type { CompactionView, QuestionView, ToolStep, TurnUserView, TurnViewData } from './turnViews';
export { deriveTurnViews } from './turnViews';

export const PAGE_LAST = 5;
export const PAGE_OLDER = 20;
const FIRST_INDEX_BASE = 1_000_000;

export function dayLabelOf(seconds: number | null): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  const now = new Date(2026, 9, 27, 14, 5, 0);
  const sameDay = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
  const clock = String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
  if (sameDay(date, now)) return '今天 ' + clock;
  const yesterday = new Date(now.getTime() - 86400000);
  if (sameDay(date, yesterday)) return '昨天 ' + clock;
  return date.getMonth() + 1 + '月' + date.getDate() + '日 ' + clock;
}

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

/** T012.9：会话合计（服务 status.stats 给；页面不自己加）。 */
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

/** T012.9：显示币种与汇率（服务 settings.json + status.fx；页面只显示和切换）。 */
export interface MoneyState {
  currency: Currency;
  cnyPerUsd: number;
  /** 汇率拉到的时间（unix 秒；0 = 没取到，用 7.2）。 */
  fxAt: number;
}

const DEFAULT_MONEY: MoneyState = { currency: 'usd', cnyPerUsd: 7.2, fxAt: 0 };

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
  /** T008.4：还没有接模型（空库）；true 时 Kind 首屏是第一次填 key。 */
  firstUse: boolean;
  /** 根会话不可用时的整页说明（T007.10）；ok = 正常对话。 */
  gate: GateState | 'ok';
  gateDetail: GateDetail | null;
  recheck: () => Promise<void>;
  /** 和内核的连接：断了就自动重连，期间不能发。 */
  connection: { state: 'online' } | { state: 'reconnecting'; attempt: number };
  /** T013.8：会话 id（ready 帧给；草稿 localStorage 的键用它）。 */
  sessionId: string | null;
  /** 上下文占用（%），底栏悬停可见。 */
  usage: number;
  compacting: boolean;
  /** 演示用：下一条消息送不到。 */
  /** T010.3：模型 id → 显示名（来自模型设置；拿不到就用 id）。 */
  modelNames: Record<string, string>;
  /** T010.4：模型 id → contextWindow（模型设置里的窗口；拿不到就不显示压缩占用）。 */
  modelWindows: Record<string, number>;
  /** T009.2：模型 id → 连接显示名（错误原因里用；取不到写「模型服务」）。 */
  connectionNames: Record<string, string>;
  /** 当前用的模型（底栏的模型选择，T008；T012.9 加价格/窗口/思考档）。 */
  model: {
    connectionId: string;
    modelId: string;
    label: string;
    /** 生效价格（美元/百万 token，已按连接倍率算好）；没有价格就是 undefined。 */
    price?: { input: number; output: number; cacheRead?: number; cacheWrite?: number };
    contextWindow?: number;
    thinks?: boolean;
    /** 这个模型能选的档（厂商自己的叫法）；没有就是不能调。 */
    efforts?: string[];
    /** 下一轮实际要发的档（T012 F2：服务端内核算好的实际档；没有想要的档时是最近档）。 */
    effort?: string;
    /** T012 F2：用户要的档（选择器显示实际档，提示时对比）。 */
    effortActual?: string;
  };
  /** T012.9：用户最后选的档（换到不能调的模型也记着）。 */
  wantedEffort?: string;
  /** T012.9：会话合计（服务 status.stats）。 */
  stats: SessionStats;
  /** T012 F2：模型信息是否已从服务初始化过（之后 refreshStats 不覆盖用户的选择，避免异步竞态）。 */
  modelInitialized: boolean;
  /** T012.9：币种与汇率（服务 settings.json + status.fx）。 */
  money: MoneyState;
  /** T008.4：Kind 首屏引导：先看是不是空库（首次使用），否则加载对话。 */
  bootstrap: () => Promise<void>;
  loadInitial: () => Promise<void>;
  loadOlder: () => Promise<void>;
  setAtBottom: (value: boolean) => void;
  send: (
    text: string,
    options?: {
      pastes?: string[];
      attachments?: { id: string; name: string; bytes: number }[];
      /** T015.4：画布放进来的那一句带着画布身份（id + 标题）。 */
      canvas?: { id: string; title: string };
    },
  ) => Promise<boolean>;
  resend: (turnId: string) => void;
  stop: () => void;
  answer: (turnId: string, answers: AskAnswer[]) => void;
  dismiss: (turnId: string) => void;
  compact: () => void;
  setModel: (model: KindState['model']) => void;
  /** T012.9：从服务刷新会话合计/汇率/币种。 */
  refreshStats: () => Promise<void>;
  /** T012.9：切币种（写 settings.json）。 */
  setCurrency: (currency: Currency) => Promise<void>;
  /** T012.9：换思考档（下一轮生效）。 */
  setEffort: (effort: string) => Promise<void>;
  /** T012 F2：应用一次服务端选择（wanted/actual 都由服务给），补目录字段并提示。 */
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
  /** T012.9：选中模型后补目录字段（档位/价格/窗口），让档位选择立刻可用。 */
  hydrateModel: (selection: {
    connectionId: string;
    modelId: string;
    label: string;
    effort?: string;
  }) => Promise<void>;
  /** T015.2：每个画布出过的版本和最新版在哪一轮（从日志折叠）。 */
  canvasTracks: Record<string, CanvasTrack>;
  /** T015.5：正在流式收到的画布标题（工作过程里画「正在画 · 标题」）；没在画是 null。 */
  drawingCanvas: { turnId: string; title: string } | null;
  /** T015.2：点了旧版画布那一行要跳到的那一轮（只影响滚动，不改日志）。 */
  reveal: { turnId: string; at: number } | null;
  revealTurn: (turnId: string) => void;
}

function indexOf(views: TurnViewData[]): Map<string, TurnViewData> {
  const map = new Map<string, TurnViewData>();
  for (const view of views) map.set(view.id, view);
  return map;
}

function applyRootState(root: {
  state: string;
  kind?: string;
  path?: string;
  line?: number;
}): void {
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
  useKindStore.setState({ gate: mapped, gateDetail: { path: root.path, line: root.line } });
}

/** T007.9：当前连接 + 重连状态；lastSeq 是页面已处理到的日志序号（重连补齐去重用）。 */
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
      reasoning: null,
      tools: [],
      cards: [],
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
  const socket = connectSession(
    lastSeq,
    (frame) => {
      if (frame.type === 'ready') {
        applyRootState(frame.root);
        const reconnects = reconnectAttempt;
        reconnectAttempt = 0;
        if (reconnects > 0) {
          // T011.6：重连成功后把「断过几次」报给服务记监控。
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
      // T007 #1：断线 5 秒还没连回来时做一次 recheck，把「服务换了令牌」与「服务连不上」分开：
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
}

/** T007.3/4：本轮已收到的日志事件（首屏页 + live）；实时帧按轮重算 view。 */
let liveEvents: ServiceEvent[] = [];
let contextWindow = 0;
let pendingCompactionAuto = true;
let livePendingCompaction: { id: string; window?: number } | null = null;
(window as unknown as { __derive?: unknown }).__derive = (events: ServiceEvent[]) => deriveTurnViews(events);
let liveTurn: number | null = null;
let liveStream = '';
/** T015.5：这一轮正在流的工具参数（callId → 名字 + 拼起来的参数）；只给「正在画」用，不落日志。 */
const liveToolCalls = new Map<string, { name: string; arguments: string }>();

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

/** 一轮的最新日志事件 → 重建这一个 view（其它轮不动）。 */
function refreshTurn(turn: number): void {
  const events = liveEvents.filter(
    (event) => typeof event.data.turn === 'number' && Number(event.data.turn) === turn,
  );
  const derived = deriveTurnViews(events)[0];
  if (!derived) return;
  const existing = useKindStore.getState().byId.get(derived.id);
  const view =
    derived.compaction || !existing?.compaction
      ? derived
      : { ...derived, compaction: existing.compaction };
  (window as unknown as { __q?: unknown }).__q = {
    turn,
    questions: view.question && 'questions' in view.question ? view.question.questions.map((item) => item.question) : null,
    status: view.question?.status,
    asked: liveEvents.filter((item) => item.type === 'question/asked').map((item) => item.data.question),
  };
  useKindStore.setState((state) => {
    const views = state.views.slice();
    const index = views.findIndex((item) => item.id === view.id);
    if (index >= 0) {
      views[index] = view;
    } else {
      views.push(view);
      views.sort((left, right) => Number(left.id.split('-')[1]) - Number(right.id.split('-')[1]));
    }
    return {
      views,
      byId: indexOf(views),
      ...decorate(views),
      streamingTurnId: view.status === 'inProgress' ? view.id : null,
    };
  });
}

/** 压缩摘要：ContentBlock 数组里的 text 拼起来（日志事实）。 */
/** 日志事件 live：追加进本轮事件，重建 view；turn/end 时收掉流式文本。 */
function applyLiveEvent(event: ServiceEvent): void {
  if (event.seq <= lastSeq) return;
  lastSeq = event.seq;
  liveEvents.push(event);
  // T015.2：画布版本从日志折叠（旧版缩成一行、标题栏切版本、点旧版跳最新都用它）。
  if (event.type === 'tool/result' && event.data.meta) {
    useKindStore.setState((state) => ({
      canvasTracks: mergeCanvasTracks(state.canvasTracks, [event]),
    }));
  }
  // T015.5：工具结果回来，这一版已经能显示了；「正在画」那一行收掉（参数增量也不再留）。
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
        reasoning: null,
        tools: [],
        cards: [],
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
  // T010.4：压缩后第一条 assistant/message 算「压缩后」占用。
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
  }
  if (event.type === 'turn/end') {
    liveStream = '';
    liveToolCalls.clear();
    useKindStore.setState({ drawingCanvas: null });
    useStreamStore.setState({ turnId: null, itemId: null, text: '' });
    // T012.9：一轮结束后从服务刷新统计（轮/步/token/花费由内核从日志算好）。
    void useKindStore.getState().refreshStats();
  }
  refreshTurn(turn);
}

/** agent 帧 live：流式文本先写进 useStreamStore（TurnRow 的 Answer 只重绘这一条）。 */
function applyAgentEvent(event: AgentFrame['event']): void {
  // T015.5：canvas 的参数增量到了就先画「正在画 · 标题」；title 可能还没流到，先只画「正在画」。
  // 帧不落日志（日志里只有 assistant stream 的 tool-call-chunks 记录）。
  if (event.type === 'tool-call-delta') {
    if (typeof event.callId !== 'string' || !event.callId) return;
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
    // T009.5：记住宿主的 home，页面显示路径时缩写成 ~/…。
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

/** 从日志事实派生界面的一轮（T007.3：打开/历史/往上翻；其余能力逐票补）。 */
const DEMO_MODEL = { connectionId: 'deepseek', modelId: 'deepseek-chat', label: 'DeepSeek Chat' };

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
  usage: 46,
  compacting: false,
  modelNames: {},
  modelWindows: {},
  connectionNames: {},
  model: DEMO_MODEL,
  stats: EMPTY_STATS,
  money: DEFAULT_MONEY,
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
      set({ modelNames: names, modelWindows: windows, connectionNames });
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
    // T012.9：统计/模型信息异步补（不让目录网络请求拖慢页面首屏）。
    void get().refreshStats();
  },

  loadInitial: async () => {
    try {
      const page = await fetchEventsPage({ maxMessages: 100, minMessages: 10, minTurns: 5 });
      const views = deriveTurnViews(page.events);
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
      const older = deriveTurnViews(page.events);
      const oldestSeq = page.events[0]?.seq ?? state.oldestSeq;
      set((current) => {
        const views = older.concat(current.views);
        return {
          views,
          byId: indexOf(views),
          ...decorate(views),
          canvasTracks: mergeCanvasTracks(current.canvasTracks, page.events),
          firstItemIndex: current.firstItemIndex - older.length,
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

  send: (text, options) => {
    // T007.4：真实发送；requestId 是幂等键（T007.9 的重发复用同一个）。
    // T013.7：pastes/attachments 跟着发（服务端拼成用户消息的块）。
    // T013.8：返回是否受理成功（草稿只在成功时清）。
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
    // T007.5：真实取消；页面状态由日志 turn/end(aborted) 驱动（不本地猜）。
    void cancelTurn().catch(() => {
      toast('停不下来', { description: '连上内核再试；或等这一轮自己结束' });
    });
  },

  answer: (_turnId, answers) => {
    // T007.6：整批真实回答；问题卡状态由 question/answered 事件回填。
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
    // T007.6：整批不答；模型收到用户没答后照常收尾。
    void dismissQuestions().catch(() => {
      toast('没送达', { description: '连上之后再试一次' });
    });
  },

  compact: () => {
    // T007.8：真实手动压缩；分隔线由 compaction/* 日志事件回填（页面不预览假摘要）。
    void compactThread(globalThis.crypto.randomUUID()).catch(() => {
      toast('压缩没成功', { description: '连上内核再试；原文都还在日志里' });
    });
  },

  setModel: (model) => {
    set((state) => ({ model: { ...state.model, ...model } }));
    // T012.9：换模型后马上补目录字段（档位/价格/窗口），不等下一轮 refreshStats。
    void get().hydrateModel({
      connectionId: model.connectionId,
      modelId: model.modelId,
      label: model.label,
      effort: model.effort,
    });
    toast('下一轮开始用 ' + model.label);
  },

  applySelection: async (selection, announce = true) => {
    const { connectionId, modelId, label, effortWanted, effortActual } = selection;
    const wanted = effortWanted ?? get().wantedEffort;
    const actual = effortActual ?? effortWanted;
    set((state) => ({
      wantedEffort: wanted,
      model: {
        ...state.model,
        connectionId,
        modelId,
        label,
        effort: actual,
        effortActual: actual,
      },
    }));
    await get().hydrateModel({ connectionId, modelId, label, effort: actual });
    if (!announce) return;
    // T012 F2：提示带档；实际档和想要的档对不上时写明「没有」。
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
        models = (await fetchAvailableModels(connectionId)).models;
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
          info?.priceInput !== undefined
            ? {
                input: info.priceInput,
                output: info.priceOutput ?? info.priceInput,
                cacheRead: info.priceCacheRead,
                cacheWrite: info.priceCacheWrite,
              }
            : undefined,
        contextWindow: info?.contextWindow,
        thinks: info?.thinks,
        efforts: info?.efforts,
        effort: selection.effort ?? state.model.effort,
        effortActual: selection.effort ?? state.model.effortActual,
      },
    }));
  },

  refreshStats: async () => {
    try {
      const status = await fetchStatus();
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
      if (status.fx || status.currency) {
        set((state) => ({
          money: {
            currency: (status.currency as Currency | undefined) ?? state.money.currency,
            cnyPerUsd: status.fx?.rate ?? state.money.cnyPerUsd,
            fxAt: status.fx?.fetchedAt ?? state.money.fxAt,
          },
        }));
      }
      // T012.9：当前模型（连接/模型/档由服务 status 给；价格/窗口/档位来自模型目录）。
      // T012 F2：只在首次初始化时写 model；之后由 applySelection 维护，避免 refreshStats 异步回来覆盖用户刚做的选择。
      if (status.stats && !get().modelInitialized) {
        const stats = status.stats;
        let models = catalogCache.get(stats.connection);
        if (models === undefined) {
          try {
            const catalog = await fetchAvailableModels(stats.connection);
            models = catalog.models;
            catalogCache.set(stats.connection, models);
          } catch {
            models = [];
          }
        }
        const info = models.find((model) => model.id === stats.model);
        set({
          model: {
            connectionId: stats.connection,
            modelId: stats.model,
            label: info?.name ?? stats.model,
            price:
              info?.priceInput !== undefined
                ? {
                    input: info.priceInput,
                    output: info.priceOutput ?? info.priceInput,
                    cacheRead: info.priceCacheRead,
                    cacheWrite: info.priceCacheWrite,
                  }
                : undefined,
            contextWindow: info?.contextWindow,
            thinks: info?.thinks,
            efforts: info?.efforts,
            effort: stats.effortActual ?? stats.effortWanted,
            effortActual: stats.effortActual,
          },
          wantedEffort: stats.effortWanted,
          modelInitialized: true,
        });
      }
    } catch {
      // 状态读不到时保留旧值（页面不显示假数据）。
    }
  },

  setCurrency: async (currency) => {
    try {
      const saved = (await updateSettings({ currency })) as Currency;
      set((state) => ({ money: { ...state.money, currency: saved } }));
    } catch {
      toast('币种没保存', { description: '连上内核再试一次' });
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
      // T012 F2：实际档由服务给（模型没有这一档时是最近档）；换档不弹提示。
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
/** T012.9：连接 id → 模型目录（第一次用到时拉一次；价格/窗口/思考档从这里来）。 */
const catalogCache = new Map<string, ServiceCatalogModel[]>();

/** 流式文本单独放一个 store：只有最后一条在重绘，别的消息不跟着重建。 */
export const useStreamStore = create<{ turnId: string | null; itemId: string | null; text: string }>(() => ({
  turnId: null,
  itemId: null,
  text: '',
}));

// T008 r4 #1：任何调用拿到 401（service 请求层唯一判断）→ 令牌说明页，先于第一次使用页。
setUnauthorizedHandler(() => {
  useKindStore.setState({
    gate: 'token',
    gateDetail: null,
    firstUse: false,
    initialLoading: false,
  });
  go('kind:conversation');
});
