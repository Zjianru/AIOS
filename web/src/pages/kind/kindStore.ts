/**
 * Kind 对话的状态与分页（需求 §三.3）：
 * - 打开时只取最后一页（5 轮，sortDirection: 'desc'），照 web/server/bridge.mjs:887 的做法；
 * - 往上滚到顶再取更早的，每页 20 轮，保持当前阅读位置（Virtuoso 的 firstItemIndex）；
 * - 列表里不出现「在数组里找」：进列表前就把每轮整理成 view model，按 id 建索引。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import { answerQuestions, cancelTurn, compactThread, connectSession, dismissQuestions, fetchEventsPage, fetchFirstUse, recheckRoot, reportMonitor, sendMessage, setUnauthorizedHandler } from '@/data/service';
import type { AgentFrame, Frame, ServiceEvent } from '@/data/service';
import { reconnectDelayMs } from '@/data/reconnect';
import type { GateDetail, GateState } from './KernelGate';
import { contextPercent, contextTokens } from '@/data/turnUsage';
import { getModelSettings } from '@/data/keyring';
import { setHostHome } from '@/data/service';
import { go } from '@/app/navigate';
import type { AskAnswer } from '@/data/fixtures/kindLive';
import { mark } from '@/lib/perf';
import { compactionViewOf, deriveTurnViews } from './turnViews';
import type { TurnViewData } from './turnViews';
export type { CompactionView, QuestionView, ToolStep, TurnViewData } from './turnViews';
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
  /** 当前用的模型（底栏的模型选择，T008）。 */
  model: { connectionId: string; modelId: string; label: string };
  /** T008.4：Kind 首屏引导：先看是不是空库（首次使用），否则加载对话。 */
  bootstrap: () => Promise<void>;
  loadInitial: () => Promise<void>;
  loadOlder: () => Promise<void>;
  setAtBottom: (value: boolean) => void;
  send: (text: string, totalMs?: number) => void;
  resend: (turnId: string) => void;
  stop: () => void;
  answer: (turnId: string, answers: AskAnswer[]) => void;
  dismiss: (turnId: string) => void;
  compact: () => void;
  setModel: (model: KindState['model']) => void;
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
/** 没送到内核的本地消息：viewId -> { requestId, text }；重发复用同一个 requestId。 */
const unsentSends = new Map<string, { requestId: string; text: string }>();

function removeView(viewId: string): void {
  useKindStore.setState((state) => {
    const views = state.views.filter((view) => view.id !== viewId);
    return { views, byId: indexOf(views) };
  });
}

/** 发不出去时在对话里留一条「没送到内核」的本地消息（重发成功后移除）。 */
function addUnsentView(viewId: string, text: string): void {
  useKindStore.setState((state) => {
    if (state.views.some((view) => view.id === viewId)) return {};
    const view: TurnViewData = {
      id: viewId,
      startedAt: null,
      durationMs: null,
      status: 'completed',
      user: { text, elements: [] },
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
        useKindStore.setState({ connection: { state: 'online' } });
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
    useStreamStore.setState({ turnId: null, itemId: null, text: '' });
  }
  refreshTurn(turn);
}

/** agent 帧 live：流式文本先写进 useStreamStore（TurnRow 的 Answer 只重绘这一条）。 */
function applyAgentEvent(event: AgentFrame['event']): void {
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
  usage: 46,
  compacting: false,
  modelNames: {},
  modelWindows: {},
  connectionNames: {},
  model: DEMO_MODEL,

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
      useStreamStore.setState({ turnId: null, itemId: null, text: '' });
      lastSeq = lastSeqOfPage;
      openSocket();
      mark('turns:loaded');
      set({
        views,
        byId: indexOf(views),
        ...decorate(views),
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

  send: (text) => {
    // T007.4：真实发送；requestId 是幂等键（T007.9 的重发复用同一个）。
    const requestId = globalThis.crypto.randomUUID();
    const viewId = 'turn-999-unsent-' + requestId;
    void sendMessage(text, requestId).catch(() => {
      unsentSends.set(viewId, { requestId, text });
      addUnsentView(viewId, text);
    });
  },

  resend: (turnId) => {
    const pending = unsentSends.get(turnId);
    if (!pending) return;
    const viewId = turnId;
    void sendMessage(pending.text, pending.requestId)
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
    set({ model });
    toast('下一轮开始用 ' + model.label);
  },

}));
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
