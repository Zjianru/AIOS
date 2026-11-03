/**
 * Kind 对话的状态与分页（需求 §三.3）：
 * - 打开时只取最后一页（5 轮，sortDirection: 'desc'），照 web/server/bridge.mjs:887 的做法；
 * - 往上滚到顶再取更早的，每页 20 轮，保持当前阅读位置（Virtuoso 的 firstItemIndex）；
 * - 列表里不出现「在数组里找」：进列表前就把每轮整理成 view model，按 id 建索引。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import { compactionSummary, liveDemo } from '@/data/api';
import { answerQuestions, cancelTurn, compactThread, connectSession, dismissQuestions, fetchEventsPage, sendMessage } from '@/data/service';
import type { AgentFrame, Frame, ServiceEvent } from '@/data/service';
import type { AskAnswer, AskQuestion, FailureCause, LiveDemo, LiveScript, LiveStep } from '@/data/fixtures/kindLive';
import { mark } from '@/lib/perf';
import type { KindCard, TextElement, Turn, TurnStatus } from '@/data/types';

export const PAGE_LAST = 5;
export const PAGE_OLDER = 20;
const FIRST_INDEX_BASE = 1_000_000;

export interface ToolStep {
  id: string;
  command: string;
  output: string;
  exitCode: number | null;
  durationMs: number | null;
  tool?: string;
  /** 这一步还在跑（运行中的一轮里实时出现）。 */
  running?: boolean;
}

/** 模型提问：等你答 → 答了 / 不答 / 这一轮被停了。 */
export type QuestionView =
  | { status: 'pending'; questions: AskQuestion[] }
  | { status: 'answered'; questions: AskQuestion[]; answers: AskAnswer[] }
  | { status: 'dismissed'; questions: AskQuestion[] }
  | { status: 'void'; questions: AskQuestion[] };

/** 压缩：在对话里是一条分隔线，不是一轮。 */
export interface CompactionView {
  auto: boolean;
  before: number;
  after: number;
  summary: string;
}

export interface TurnViewData {
  id: string;
  startedAt: number | null;
  durationMs: number | null;
  status: TurnStatus;
  user: { text: string; elements: TextElement[] } | null;
  reasoning: { summary: string[]; full: string; seconds: number } | null;
  tools: ToolStep[];
  cards: KindCard[];
  answer: { text: string; elements: TextElement[] } | null;
  thinkingSeconds: number;
  tokens: { input: number; output: number };
  cost: number | null;
  model: string;
  /** 模型正在重试（第几次、为什么）。 */
  retry: { attempt: number; reason: string; inSeconds: number } | null;
  /** 这一轮没做完：原因来自内核，按原因给下一步（去设置 / 再发一次）。 */
  failure: { cause: FailureCause; reason: string } | null;
  question: QuestionView | null;
  compaction: CompactionView | null;
  /** 你的消息：发送中 / 没送到（重发用同一个请求编号，内核只收一次）。 */
  delivery: 'sending' | 'unsent' | null;
  /** 停下的时间（status 为 interrupted 时）。 */
  stoppedAt: string | null;
}

/** 日期分隔的文案：今天 / 昨天 / 10月26日，后面带时分。 */
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

function describeArguments(value: unknown): string {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return '';
  const record = value as Record<string, unknown>;
  const first = Object.values(record)[0];
  return typeof first === 'string' ? first : '';
}

export function buildTurnView(turn: Turn): TurnViewData {
  let user: TurnViewData['user'] = null;
  let reasoning: TurnViewData['reasoning'] = null;
  let answer: TurnViewData['answer'] = null;
  const tools: ToolStep[] = [];
  const cards: KindCard[] = [];
  for (const item of turn.items) {
    switch (item.type) {
      case 'userMessage': {
        const input = item.content[0];
        if (input && input.type === 'text') user = { text: input.text, elements: input.textElements ?? [] };
        break;
      }
      case 'reasoning':
        reasoning = {
          summary: item.summary,
          full: item.content.join(String.fromCharCode(10) + String.fromCharCode(10)),
          seconds: 3 + (item.summary.length % 6),
        };
        break;
      case 'agentMessage':
        answer = { text: item.text, elements: item.textElements ?? [] };
        break;
      case 'commandExecution':
        tools.push({
          id: item.id,
          command: item.command,
          output: item.aggregatedOutput ?? '',
          exitCode: item.exitCode,
          durationMs: item.durationMs,
        });
        break;
      case 'mcpToolCall':
        tools.push({
          id: item.id,
          command: item.tool + '(' + describeArguments(item.arguments) + ')',
          output: '',
          exitCode: 0,
          durationMs: item.durationMs,
          tool: item.tool,
        });
        break;
      case 'agentCard':
        cards.push(item.card);
        break;
      default:
        break;
    }
  }
  const duration = turn.durationMs ?? 0;
  const tokens = {
    input: 1800 + ((duration * 7) % 4200),
    output: 120 + ((duration * 3) % 900),
  };
  return {
    id: turn.id,
    startedAt: turn.startedAt,
    durationMs: turn.durationMs,
    status: turn.status,
    user,
    reasoning,
    tools,
    cards,
    answer,
    thinkingSeconds: reasoning ? reasoning.seconds : 2 + (duration % 5),
    tokens,
    cost: 0.000008 * (tokens.input + tokens.output),
    model: 'DeepSeek V4 Pro',
    retry: null,
    failure: turn.status === 'failed' ? { cause: 'model', reason: turn.error?.message ?? '原因不明' } : null,
    question: null,
    compaction: null,
    delivery: null,
    stoppedAt: null,
  };
}

interface KindState {
  views: TurnViewData[];
  byId: Map<string, TurnViewData>;
  firstItemIndex: number;
  nextCursor: string | null;
  /** 当前最老一条日志的 seq（往上翻时 beforeSeq = oldestSeq - 1）。 */
  oldestSeq: number | null;
  hasOlder: boolean;
  loadingOlder: boolean;
  initialLoading: boolean;
  atBottom: boolean;
  streamingTurnId: string | null;
  dayLabels: Map<string, string>;
  lastSeenId: string | null;
  lastSeenCounts: { moved: number; waiting: number };
  /** 和内核的连接：断了就自动重连，期间不能发。 */
  connection: { state: 'online' } | { state: 'reconnecting'; attempt: number };
  /** 上下文占用（%），底栏悬停可见。 */
  usage: number;
  compacting: boolean;
  /** 演示用：下一条消息送不到。 */
  nextSendFails: boolean;
  /** 当前用的模型（底栏的模型选择，T008）。 */
  model: { connectionId: string; modelId: string; label: string };
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
  demo: (name: LiveDemo | 'compactAuto' | 'offline' | 'unsent' | 'empty' | 'restore') => void;
}

function indexOf(views: TurnViewData[]): Map<string, TurnViewData> {
  const map = new Map<string, TurnViewData>();
  for (const view of views) map.set(view.id, view);
  return map;
}

function clock(date = new Date()): string {
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

function emptyTurn(id: string, text: string): TurnViewData {
  const stamp = Date.now();
  return {
    ...buildTurnView({
      id,
      items: [],
      itemsView: 'full',
      status: 'inProgress',
      error: null,
      startedAt: Math.floor(stamp / 1000),
      completedAt: null,
      durationMs: null,
    }),
    user: { text, elements: [] },
    model: useKindStore.getState().model.label,
    tokens: { input: 0, output: 0 },
    cost: null,
    thinkingSeconds: 0,
  };
}

/** T007.9：当前连接 + 重连状态；lastSeq 是页面已处理到的日志序号（重连补齐去重用）。 */
let activeSocket: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
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
      cost: null,
      model: DEMO_MODEL.label,
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
        reconnectAttempt = 0;
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
      if (reconnectTimer === null) {
        const delay = Math.min(5_000, 300 * reconnectAttempt);
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
function summaryTextOf(data: Record<string, unknown>): string {
  const blocks = Array.isArray(data.summary) ? data.summary : [];
  return blocks
    .map((block) =>
      block && typeof block === 'object' ? String((block as { text?: unknown }).text ?? '') : '',
    )
    .join('');
}

function compactionViewOf(data: Record<string, unknown>, auto: boolean) {
  return { auto, before: 0, after: 0, summary: summaryTextOf(data) };
}

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
        cost: null,
        model: DEMO_MODEL.label,
        retry: null,
        failure: null,
        question: null,
        compaction,
        delivery: null,
        stoppedAt: null,
      };
      const exists = state.views.some((item) => item.id === id);
      const views = (exists ? state.views.map((item) => (item.id === id ? view : item)) : state.views.concat(view)).sort(
        (left, right) => Number(left.id.split('-')[1]) - Number(right.id.split('-')[1]),
      );
      return { views, byId: indexOf(views) };
    });
    return;
  }
  if (turn === null) return;
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
export function deriveTurnViews(events: ServiceEvent[]): TurnViewData[] {
  interface Raw {
    turn: number;
    user: string | null;
    answer: string | null;
    status: TurnStatus;
    tools: ToolStep[];
    tokens: { input: number; output: number };
    failure: { cause: FailureCause; reason: string } | null;
    question: QuestionView | null;
    retry: TurnViewData['retry'];
    compaction: CompactionView | null;
    answered: AskAnswer[];
    cancelled: number;
    stoppedAt: string | null;
  }
  const turns = new Map<number, Raw>();
  const ensure = (turn: number): Raw => {
    let raw = turns.get(turn);
    if (!raw) {
      raw = {
        turn,
        user: null,
        answer: null,
        status: 'inProgress',
        tools: [],
        tokens: { input: 0, output: 0 },
        failure: null,
        question: null,
        retry: null,
        compaction: null,
        answered: [],
        cancelled: 0,
        stoppedAt: null,
      };
      turns.set(turn, raw);
    }
    return raw;
  };
  const textOf = (message: unknown): string => {
    if (!message || typeof message !== 'object') return '';
    const content = (message as { content?: unknown }).content;
    if (!Array.isArray(content)) return '';
    return content
      .map((block) =>
        block && typeof block === 'object' && (block as { type?: string }).type === 'text'
          ? String((block as { text?: unknown }).text ?? '')
          : '',
      )
      .join('');
  };

  const textOfToolResult = (message: unknown): string => {
    if (!message || typeof message !== 'object') return '';
    const content = (message as { content?: unknown }).content;
    if (typeof content === 'string') return content;
    if (!Array.isArray(content)) return '';
    const parts: string[] = [];
    for (const block of content) {
      if (!block || typeof block !== 'object') continue;
      const typed = block as { type?: string; text?: unknown; content?: unknown };
      if (typed.type === 'text' && typeof typed.text === 'string') {
        parts.push(typed.text);
        continue;
      }
      if (typed.type === 'tool_result') {
        if (typeof typed.content === 'string') parts.push(typed.content);
        else if (Array.isArray(typed.content)) {
          for (const inner of typed.content) {
            if (inner && typeof inner === 'object' && (inner as { type?: string }).type === 'text') {
              parts.push(String((inner as { text?: unknown }).text ?? ''));
            }
          }
        }
      }
    }
    return parts.join('');
  };

  const usageOf = (data: Record<string, unknown>): { input: number; output: number } => {
    const usage = data.usage;
    if (!usage || typeof usage !== 'object') return { input: 0, output: 0 };
    const value = (name: string) => {
      const number = (usage as Record<string, unknown>)[name];
      return typeof number === 'number' ? number : 0;
    };
    return { input: value('inputTokens'), output: value('outputTokens') };
  };
  for (const event of events) {
    const data = event.data ?? {};
    const turn = typeof data.turn === 'number' ? data.turn : null;
    if (turn === null) continue;
    switch (event.type) {
      case 'turn/start':
        ensure(turn);
        break;
      case 'user/message': {
        const source = data.message as
          | { source?: string | { kind?: string } }
          | undefined;
        const kind =
          typeof source?.source === 'string' ? source.source : source?.source?.kind;
        if (kind !== undefined && kind !== 'user' && kind !== 'skill-invocation') break;
        const text = textOf(data.message);
        if (text) ensure(turn).user = text;
        break;
      }
      case 'assistant/message': {
        const raw = ensure(turn);
        const text = textOf(data.message);
        if (text) raw.answer = text;
        raw.tokens = usageOf(data);
        raw.status = 'inProgress';
        raw.retry = null;
        break;
      }
      case 'tool/call': {
        const raw = ensure(turn);
        raw.tools.push({
          id: String(data.call_id ?? data.callId ?? ''),
          command: String(data.name ?? ''),
          output: '',
          exitCode: null,
          durationMs: null,
          running: true,
        });
        break;
      }
      case 'tool/result': {
        const raw = ensure(turn);
        const step = [...raw.tools].reverse().find((item) => item.running);
        if (step) {
          step.running = false;
          step.exitCode = data.error ? 1 : 0;
          step.output = textOf(data.message) || textOfToolResult(data.message);
          if (!step.output) step.output = String((data.error as { code?: string } | undefined)?.code ?? '');
        }
        break;
      }
      case 'turn/end': {
        const raw = ensure(turn);
        const reason = data.reason as { kind?: string; error?: { message?: string } } | undefined;
        switch (reason?.kind) {
          case 'completed':
            raw.status = 'completed';
            break;
          case 'aborted':
          case 'interrupted':
            raw.status = 'interrupted';
            raw.stoppedAt = clock();
            if (raw.question && raw.question.status === 'pending') {
              raw.question = { status: 'void', questions: raw.question.questions };
            }
            break;
          case 'error':
            raw.status = 'failed';
            raw.failure = {
              cause: event.cause ?? 'model',
              reason: reason.error?.message ?? '原因不明',
            };
            break;
          default:
            raw.status = 'failed';
        }
        break;
      }
      case 'question/asked': {
        const raw = ensure(turn);
        if (typeof data.question !== 'string') break;
        const asked: AskQuestion = {
          id: String(data.question_id ?? data.questionId ?? ''),
          question: data.question,
          header: typeof data.header === 'string' ? data.header : undefined,
          options: Array.isArray(data.options)
            ? (data.options as string[]).map((label) => ({ label }))
            : undefined,
          multiSelect: Boolean(data.multi_select ?? data.multiSelect),
        };
        const current = raw.question;
        if (!current || current.status !== 'pending') {
          raw.question = { status: 'pending', questions: [asked] };
        } else if (!current.questions.some((question) => question.id === asked.id)) {
          current.questions.push(asked);
        }
        break;
      }
      case 'question/answered': {
        const raw = ensure(turn);
        raw.answered.push({
          id: String(data.question_id ?? data.questionId ?? ''),
          selected: Array.isArray(data.selected) ? (data.selected as string[]) : [],
          custom: typeof data.custom === 'string' ? data.custom : undefined,
        });
        const questions =
          raw.question && raw.question.status !== 'dismissed' ? raw.question.questions : [];
        if (questions.length > 0 && raw.answered.length >= questions.length) {
          raw.question = { status: 'answered', questions, answers: raw.answered };
        } else if (questions.length > 0) {
          raw.question = { status: 'pending', questions };
        }
        break;
      }
      case 'question/cancelled': {
        const raw = ensure(turn);
        raw.cancelled += 1;
        const questions =
          raw.question && raw.question.status !== 'answered' ? raw.question.questions : [];
        if (questions.length > 0 && raw.cancelled >= questions.length) {
          raw.question = { status: 'dismissed', questions };
        }
        break;
      }
      case 'llm/retry': {
        const raw = ensure(turn);
        const delayMs = typeof data.delayMs === 'number' ? data.delayMs : typeof data.delay_ms === 'number' ? data.delay_ms : 0;
        const failure = (data.failure ?? {}) as { message?: string; code?: string };
        raw.retry = {
          attempt: typeof data.retry === 'number' ? data.retry : 1,
          reason: failure.message || failure.code || '模型请求失败',
          inSeconds: Math.max(1, Math.round(delayMs / 1000)),
        };
        break;
      }
      case 'llm/retry-started': {
        ensure(turn).retry = null;
        break;
      }
      case 'compaction/summary': {
        const raw = ensure(turn);
        const auto =
          data.sourceCommandId === undefined || data.sourceCommandId === null;
        raw.compaction = compactionViewOf(data, auto);
        break;
      }
      default:
        break;
    }
  }
  return [...turns.values()]
    .sort((left, right) => left.turn - right.turn)
    .map((raw) => ({
      id: `turn-${raw.turn}`,
      startedAt: null,
      durationMs: null,
      status: raw.status,
      user: raw.user ? { text: raw.user, elements: [] } : null,
      reasoning: null,
      tools: raw.tools,
      cards: [],
      answer: raw.answer ? { text: raw.answer, elements: [] } : null,
      thinkingSeconds: 0,
      tokens: raw.tokens,
      cost: null,
      model: DEMO_MODEL.label,
      retry: raw.retry,
      compaction: raw.compaction,
      failure: raw.failure,
      question: raw.question,
      delivery: null,
      stoppedAt: raw.stoppedAt,
    }));
}

/** 改一轮：按 id 换成新对象（TurnRow 用 memo 比引用，只有这一条重绘）。 */
function patch(id: string, change: (view: TurnViewData) => TurnViewData): void {
  useKindStore.setState((state) => {
    const index = state.views.findIndex((view) => view.id === id);
    if (index < 0) return {};
    const views = state.views.slice();
    views[index] = change(views[index]);
    return { views, byId: indexOf(views) };
  });
}

function append(view: TurnViewData): void {
  useKindStore.setState((state) => {
    const views = state.views.concat(view);
    return { views, byId: indexOf(views), ...decorate(views), atBottom: true };
  });
}

/* ───────── 一轮的演示：按步骤走（真的由内核推事件，这里只模拟样子） ───────── */

interface Run {
  turnId: string;
  steps: LiveStep[];
  index: number;
  startedAt: number;
  timer: ReturnType<typeof setTimeout> | ReturnType<typeof setInterval> | null;
  waiting: boolean;
}

let run: Run | null = null;

function clearTimer(): void {
  if (run?.timer) {
    clearTimeout(run.timer as ReturnType<typeof setTimeout>);
    clearInterval(run.timer as ReturnType<typeof setInterval>);
    run.timer = null;
  }
}

function startRun(turnId: string, script: LiveScript): void {
  run = { turnId, steps: script.steps, index: 0, startedAt: Date.now(), timer: null, waiting: false };
  mark('stream:start');
  useKindStore.setState({ streamingTurnId: turnId });
  next();
}

function finish(status: TurnStatus): void {
  if (!run) return;
  const { turnId, startedAt } = run;
  clearTimer();
  run = null;
  const durationMs = Date.now() - startedAt;
  patch(turnId, (view) => ({
    ...view,
    status,
    durationMs,
    retry: null,
    tokens: { input: 2400 + (durationMs % 3000), output: 180 + (durationMs % 600) },
    thinkingSeconds: Math.max(1, Math.round(durationMs / 4000)),
  }));
  useKindStore.setState((state) => ({ streamingTurnId: null, usage: Math.min(99, state.usage + 3) }));
  mark('stream:end');
}

function next(): void {
  if (!run) return;
  const current = run;
  const step = current.steps[current.index++];
  if (!step) {
    finish('completed');
    return;
  }
  const id = current.turnId;
  switch (step.kind) {
    case 'tool': {
      const toolId = id + '_t' + current.index;
      const begun = Date.now();
      patch(id, (view) => ({
        ...view,
        tools: view.tools.concat({ id: toolId, command: step.command, output: '', exitCode: null, durationMs: null, running: true }),
      }));
      current.timer = setTimeout(() => {
        patch(id, (view) => ({
          ...view,
          tools: view.tools.map((tool) =>
            tool.id === toolId ? { ...tool, running: false, output: step.output, exitCode: 0, durationMs: Date.now() - begun } : tool,
          ),
        }));
        next();
      }, step.ms);
      return;
    }
    case 'retry': {
      patch(id, (view) => ({ ...view, retry: { attempt: step.attempt, reason: step.reason, inSeconds: Math.round(step.ms / 1000) } }));
      current.timer = setTimeout(() => {
        patch(id, (view) => ({ ...view, retry: null }));
        next();
      }, step.ms);
      return;
    }
    case 'ask': {
      current.waiting = true;
      patch(id, (view) => ({ ...view, question: { status: 'pending', questions: step.questions } }));
      return;
    }
    case 'say': {
      const text = step.text;
      const chunk = Math.max(1, Math.ceil(text.length / 90));
      const tick = Math.max(16, Math.round(step.ms / (text.length / chunk)));
      let offset = 0;
      useStreamStore.setState({ turnId: id, itemId: id + '_say', text: '' });
      current.timer = setInterval(() => {
        offset = Math.min(text.length, offset + chunk);
        useStreamStore.setState({ text: text.slice(0, offset) });
        if (offset >= text.length) {
          clearTimer();
          patch(id, (view) => ({ ...view, answer: { text, elements: [] } }));
          useStreamStore.setState({ turnId: null, itemId: null, text: '' });
          next();
        }
      }, tick);
      return;
    }
    case 'fail': {
      patch(id, (view) => ({ ...view, failure: { cause: step.cause, reason: step.reason } }));
      finish('failed');
      return;
    }
  }
}

/** 你的消息先「发送中」，内核收下后这一轮才开始。 */
function deliver(turnId: string, script: LiveScript): void {
  const state = useKindStore.getState();
  if (state.nextSendFails) {
    useKindStore.setState({ nextSendFails: false });
    setTimeout(() => patch(turnId, (view) => ({ ...view, delivery: 'unsent', status: 'interrupted' })), 900);
    pendingScripts.set(turnId, script);
    return;
  }
  setTimeout(() => {
    patch(turnId, (view) => ({ ...view, delivery: null }));
    startRun(turnId, script);
  }, 350);
}

/** 没送到的消息，重发时还用原来那一轮的脚本。 */
const pendingScripts = new Map<string, LiveScript>();

function begin(script: LiveScript): void {
  if (run) return;
  const replyId = 'local_reply_' + Date.now();
  append({ ...emptyTurn(replyId, script.user), delivery: 'sending' });
  deliver(replyId, script);
}

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
  connection: { state: 'online' },
  usage: 46,
  compacting: false,
  nextSendFails: false,
  model: DEMO_MODEL,

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
      set({ initialLoading: false });
    }
  },

  loadOlder: async () => {
    const state = get();
    if (!state.hasOlder || state.loadingOlder || state.oldestSeq === null) return;
    set({ loadingOlder: true });
    mark('prepend:start');
    try {
      const page = await fetchEventsPage({
        beforeSeq: state.oldestSeq - 1,
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

  demo: (name) => {
    switch (name) {
      case 'compactAuto': {
        const before = 92;
        append({ ...emptyTurn('compact_' + Date.now(), ''), user: null, status: 'completed', compaction: { auto: true, before, after: 11, summary: compactionSummary() } });
        set({ usage: 11 });
        return;
      }
      case 'offline': {
        set({ connection: { state: 'reconnecting', attempt: 1 } });
        setTimeout(() => set({ connection: { state: 'reconnecting', attempt: 2 } }), 1800);
        setTimeout(() => {
          set({ connection: { state: 'online' } });
          toast('已经重新连上', { description: '断开期间的 12 条更新已经补上' });
        }, 4200);
        return;
      }
      case 'unsent':
        set({ nextSendFails: true });
        toast('下一条消息会送不到（演示）');
        return;
      case 'empty':
        if (run) get().stop();
        set({ views: [], byId: new Map(), ...decorate([]), hasOlder: false, nextCursor: null, usage: 0 });
        return;
      case 'restore':
        set({ initialLoading: true, usage: 46 });
        void get().loadInitial();
        return;
      default:
        begin(liveDemo(name));
    }
  },
}));
/** 流式文本单独放一个 store：只有最后一条在重绘，别的消息不跟着重建。 */
export const useStreamStore = create<{ turnId: string | null; itemId: string | null; text: string }>(() => ({
  turnId: null,
  itemId: null,
  text: '',
}));
