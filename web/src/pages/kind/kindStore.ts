/**
 * Kind 对话的状态与分页（需求 §三.3）：
 * - 打开时只取最后一页（5 轮，sortDirection: 'desc'），照 web/server/bridge.mjs:887 的做法；
 * - 往上滚到顶再取更早的，每页 20 轮，保持当前阅读位置（Virtuoso 的 firstItemIndex）；
 * - 列表里不出现「在数组里找」：进列表前就把每轮整理成 view model，按 id 建索引。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import { compactionSummary, liveDemo, liveScriptFor, threadResume, threadTurnsList } from '@/data/api';
import type { AskAnswer, AskQuestion, FailureCause, LiveDemo, LiveScript, LiveStep } from '@/data/fixtures/kindLive';
import { KIND_THREAD_ID } from '@/data/generate';
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
    const response = await threadResume({
      threadId: KIND_THREAD_ID,
      excludeTurns: true,
      initialTurnsPage: { limit: PAGE_LAST, sortDirection: 'desc', itemsView: 'full' },
    });
    // 协议给的是「新的在前」，界面按「旧 → 新」排；bridge.mjs 也是先 reverse 再用。
    const turns = response.initialTurnsPage.data.slice().reverse();
    const views = turns.map(buildTurnView);
    mark('turns:loaded');
    set({
      views,
      byId: indexOf(views),
      ...decorate(views),
      firstItemIndex: FIRST_INDEX_BASE - views.length,
      nextCursor: response.initialTurnsPage.nextCursor,
      hasOlder: Boolean(response.initialTurnsPage.nextCursor),
      initialLoading: false,
    });
  },

  loadOlder: async () => {
    const state = get();
    if (!state.hasOlder || state.loadingOlder || !state.nextCursor) return;
    set({ loadingOlder: true });
    mark('prepend:start');
    try {
      const response = await threadTurnsList({
        threadId: KIND_THREAD_ID,
        cursor: state.nextCursor,
        limit: PAGE_OLDER,
        sortDirection: 'desc',
        itemsView: 'full',
      });
      const older = response.data.slice().reverse().map(buildTurnView);
      set((current) => {
        const views = older.concat(current.views);
        return {
          views,
          byId: indexOf(views),
          ...decorate(views),
          firstItemIndex: current.firstItemIndex - older.length,
          nextCursor: response.nextCursor,
          hasOlder: Boolean(response.nextCursor),
          loadingOlder: false,
        };
      });
      requestAnimationFrame(() => requestAnimationFrame(() => mark('prepend:painted')));
    } catch {
      set({ loadingOlder: false });
    }
  },

  setAtBottom: (value) => set({ atBottom: value }),

  send: (text, totalMs) => {
    const script = liveScriptFor(text);
    if (totalMs !== undefined) {
      // 测速钩子：只量流式回答，不走工具
      script.steps = script.steps.filter((step) => step.kind === 'say').map((step) => ({ ...step, ms: totalMs }));
    }
    begin(script);
  },

  resend: (turnId) => {
    const script = pendingScripts.get(turnId);
    if (!script || run) return;
    pendingScripts.delete(turnId);
    patch(turnId, (view) => ({ ...view, delivery: 'sending', status: 'inProgress' }));
    deliver(turnId, script);
  },

  stop: () => {
    if (!run) return;
    const id = run.turnId;
    clearTimer();
    const streamed = useStreamStore.getState();
    const partial = streamed.turnId === id ? streamed.text : '';
    useStreamStore.setState({ turnId: null, itemId: null, text: '' });
    patch(id, (view) => ({
      ...view,
      answer: partial ? { text: partial, elements: [] } : view.answer,
      tools: view.tools.map((tool) => (tool.running ? { ...tool, running: false, output: '停下了' } : tool)),
      question: view.question?.status === 'pending' ? { status: 'void', questions: view.question.questions } : view.question,
      stoppedAt: clock(),
    }));
    finish('interrupted');
  },

  answer: (turnId, answers) => {
    if (!run || run.turnId !== turnId || !run.waiting) return;
    const step = run.steps[run.index - 1];
    run.waiting = false;
    patch(turnId, (view) =>
      view.question ? { ...view, question: { status: 'answered', questions: view.question.questions, answers } } : view,
    );
    if (step?.kind === 'ask') run.steps.splice(run.index, 0, { kind: 'say', text: step.answered, ms: 2600 });
    next();
  },

  dismiss: (turnId) => {
    if (!run || run.turnId !== turnId || !run.waiting) return;
    const step = run.steps[run.index - 1];
    run.waiting = false;
    patch(turnId, (view) => (view.question ? { ...view, question: { status: 'dismissed', questions: view.question.questions } } : view));
    if (step?.kind === 'ask') run.steps.splice(run.index, 0, { kind: 'say', text: step.dismissed, ms: 2200 });
    next();
  },

  compact: () => {
    const state = get();
    if (state.compacting || run) return;
    set({ compacting: true });
    setTimeout(() => {
      const before = get().usage;
      const after = 9;
      append({ ...emptyTurn('compact_' + Date.now(), ''), user: null, status: 'completed', compaction: { auto: false, before, after, summary: compactionSummary() } });
      set({ compacting: false, usage: after });
    }, 1600);
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
