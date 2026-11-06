/// T009.7：Kind 的轮次视图与从日志事件派生（只搬代码，行为不变）。

import type { SendAttachmentRef, ServiceEvent } from '@/data/service';
import type { AskAnswer, AskQuestion, FailureCause } from '@/data/fixtures/kindLive';
import type { KindCard, TextElement, TurnStatus } from '@/data/types';
import { modelErrorReason, retryReason } from '@/data/errors';
import { parseExitStatus } from './process/toolModel';
import type { ProcessStep } from './process/toolModel';
import { useKindStore } from './kindStore';
import {
  contextPercent,
  contextTokens,
  emptyUsage,
  mergeUsage,
  modelLabel,
  reasoningSpanMs,
  thinkingSeconds as thinkingSecondsOf,
  type UsageTotals,
} from '@/data/turnUsage';

export interface ToolStep {
  id: string;
  /** T013.5：工具名（read/write/bash/...）与解析后的参数。 */
  name: string;
  args: Record<string, unknown>;
  output: string;
  exitCode: number | null;
  /** 被信号杀掉时的信号名（bash 的 [killed by signal: X]）。 */
  signal?: string | null;
  durationMs: number | null;
  /** running/done/error/stopped（T013.5：停下的一轮把没结束的标 stopped）。 */
  state: 'running' | 'done' | 'error' | 'stopped';
  /** T013.1：tool/result.meta（diffs/operation/stopped）。 */
  meta?: Record<string, unknown> | null;
  /** 调用事件的时间（算这一步用时）。 */
  startedAtMs?: number | null;
  tool?: string;
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
  /** T010.4：压缩前占窗口的百分比（窗口/前后任一拿不到就不显示）。 */
  before?: number;
  /** T010.4：压缩后第一条响应占窗口的百分比。 */
  after?: number;
  summary: string;
}

/**
 * 你的消息（T013 F1）：正文只放打的字；每段粘贴、每个文件各是一张卡。
 * 块从日志的 user/message 来，实时和回放同一份。
 */
export interface TurnUserView {
  text: string;
  elements: TextElement[];
  /** paste: true 的文字块，一段一条（不拼进正文）。 */
  pastes: string[];
  /** file 块（服务发消息时写下的 id/名字/大小）。 */
  files: SendAttachmentRef[];
}

export interface TurnViewData {
  id: string;
  startedAt: number | null;
  durationMs: number | null;
  status: TurnStatus;
  user: TurnUserView | null;
  reasoning: { summary: string[]; full: string; seconds: number } | null;
  tools: ToolStep[];
  cards: KindCard[];
  answer: { text: string; elements: TextElement[] } | null;
  thinkingSeconds: number;
  tokens: { input: number; output: number; cacheRead?: number; cacheWrite?: number };
  /** T010.3：这一轮最后一条 assistant/message 的模型显示名（日志里没有就退回 id）。 */
  model: string;
  cost: number | null;
  /** 模型正在重试（第几次、为什么）。 */
  retry: { attempt: number; reason: string; inSeconds: number; code?: string } | null;
  /** 这一轮没做完：原因来自内核，按原因给下一步（去设置 / 再发一次）。 */
  failure: { cause: FailureCause; reason: string; code?: string } | null;
  question: QuestionView | null;
  compaction: CompactionView | null;
  /** 你的消息：发送中 / 没送到（重发用同一个请求编号，内核只收一次）。 */
  delivery: 'sending' | 'unsent' | null;
  /** 停下的时间（status 为 interrupted 时）。 */
  stoppedAt: string | null;
}

/** 日期分隔的文案：今天 / 昨天 / 10月26日，后面带时分。 */
function clock(date = new Date()): string {
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

/** root 状态 → 整页说明（T007.10）：路径/行号来自服务事实，不在页面上编。 */
export function summaryTextOf(data: Record<string, unknown>): string {
  const blocks = Array.isArray(data.summary) ? data.summary : [];
  return blocks
    .map((block) =>
      block && typeof block === 'object' ? String((block as { text?: unknown }).text ?? '') : '',
    )
    .join('');
}

export function compactionViewOf(data: Record<string, unknown>, auto: boolean): CompactionView {
  return { auto, summary: summaryTextOf(data) };
}

export function deriveTurnViews(events: ServiceEvent[]): TurnViewData[] {
  interface Raw {
    turn: number;
    startedAt: number | null;
    endedAt: number | null;
    user: TurnUserView | null;
    answer: string | null;
    status: TurnStatus;
    tools: ToolStep[];
    usage: UsageTotals;
    modelId: string | null;
    reasoningMs: number;
    failure: { cause: FailureCause; reason: string; code?: string } | null;
    question: QuestionView | null;
    retry: TurnViewData['retry'];
    compaction: CompactionView | null;
    answered: AskAnswer[];
    cancelled: number;
    stoppedAt: string | null;
  }
  const turns = new Map<number, Raw>();
  // T010.4：每次压缩摘要等「压缩后第一条响应」来算 after。
  const pendingCompaction = new Map<number, { view: CompactionView; window?: number }>();
  const ensure = (turn: number): Raw => {
    let raw = turns.get(turn);
    if (!raw) {
      raw = {
        turn,
        startedAt: null,
        endedAt: null,
        user: null,
        answer: null,
        status: 'inProgress',
        tools: [],
        usage: emptyUsage(),
        modelId: null,
        reasoningMs: 0,
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

  /**
   * T013 F1：你的消息按块拆开——正文只留打的字；paste 块各是一段粘贴；file 块各是一个文件。
   * 粘贴原文不进正文（Claude F1：气泡上是一排卡片，不是一大段字）。
   */
  const userOf = (message: unknown): TurnUserView | null => {
    if (!message || typeof message !== 'object') return null;
    const content = (message as { content?: unknown }).content;
    if (!Array.isArray(content)) return null;
    const text: string[] = [];
    const pastes: string[] = [];
    const files: SendAttachmentRef[] = [];
    for (const block of content) {
      if (!block || typeof block !== 'object') continue;
      const typed = block as {
        type?: string;
        text?: unknown;
        paste?: unknown;
        attachmentId?: unknown;
        name?: unknown;
        bytes?: unknown;
      };
      if (typed.type === 'text') {
        const value = typeof typed.text === 'string' ? typed.text : '';
        if (typed.paste === true) pastes.push(value);
        else text.push(value);
        continue;
      }
      if (typed.type === 'file' && typeof typed.attachmentId === 'string') {
        files.push({
          id: typed.attachmentId,
          name: typeof typed.name === 'string' ? typed.name : '',
          bytes: typeof typed.bytes === 'number' ? typed.bytes : 0,
        });
      }
    }
    const body = text.join('');
    if (!body && pastes.length === 0 && files.length === 0) return null;
    return { text: body, elements: [], pastes, files };
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

  const modelIdOf = (message: unknown): string | null => {
    if (!message || typeof message !== 'object') return null;
    const source = (message as { source?: { kind?: string; model?: unknown } }).source;
    if (!source || source.kind !== 'model') return null;
    return typeof source.model === 'string' ? source.model : null;
  };
  for (const event of events) {
    const data = event.data ?? {};
    const turn = typeof data.turn === 'number' ? data.turn : null;
    if (turn === null) continue;
    switch (event.type) {
      case 'turn/start': {
        const raw = ensure(turn);
        raw.startedAt = event.time;
        break;
      }
      case 'user/message': {
        const source = data.message as
          | { source?: string | { kind?: string } }
          | undefined;
        const kind =
          typeof source?.source === 'string' ? source.source : source?.source?.kind;
        if (kind !== undefined && kind !== 'user' && kind !== 'skill-invocation') break;
        const view = userOf(data.message);
        if (view) ensure(turn).user = view;
        break;
      }
      case 'assistant/message': {
        const raw = ensure(turn);
        const text = textOf(data.message);
        if (text) raw.answer = text;
        raw.usage = mergeUsage(raw.usage, data.usage);
        raw.modelId = modelIdOf(data.message) ?? raw.modelId;
        raw.reasoningMs += reasoningSpanMs(data.stream);
        const pending = pendingCompaction.get(turn);
        if (pending) {
          pending.view.after = contextPercent(contextTokens(data.usage), pending.window) ?? undefined;
          pendingCompaction.delete(turn);
        }
        raw.status = 'inProgress';
        raw.retry = null;
        break;
      }
      case 'assistant/attempt': {
        const raw = ensure(turn);
        raw.usage = mergeUsage(raw.usage, data.usage);
        raw.reasoningMs += reasoningSpanMs(data.stream);
        break;
      }
      case 'tool/call': {
        const raw = ensure(turn);
        let args: Record<string, unknown> = {};
        const rawArgs = data.arguments;
        if (typeof rawArgs === 'string' && rawArgs.trim()) {
          try {
            const parsed = JSON.parse(rawArgs) as unknown;
            if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) args = parsed as Record<string, unknown>;
          } catch {
            args = {};
          }
        }
        raw.tools.push({
          id: String(data.call_id ?? data.callId ?? ''),
          name: String(data.name ?? ''),
          args,
          output: '',
          exitCode: null,
          durationMs: null,
          state: 'running',
          startedAtMs: event.time,
        });
        break;
      }
      case 'tool/result': {
        const raw = ensure(turn);
        // T013.5：按 call id 配对（不再用「最后一条 running」猜），并留下 meta。
        const message = (data.message ?? {}) as Record<string, unknown>;
        const callId = String(message.toolCallId ?? message.tool_call_id ?? '');
        const step = callId ? raw.tools.find((item) => item.id === callId) : undefined;
        if (step) {
          step.state = data.error ? 'error' : 'done';
          const body = textOf(data.message) || textOfToolResult(data.message);
          step.output = body || String((data.error as { code?: string } | undefined)?.code ?? '');
          step.meta = (data.meta as Record<string, unknown> | undefined) ?? null;
          const status = parseExitStatus(step.output);
          step.exitCode = status.exitCode;
          step.signal = status.signal;
          if (step.startedAtMs) step.durationMs = Math.max(0, event.time - step.startedAtMs);
        }
        break;
      }
      case 'turn/end': {
        const raw = ensure(turn);
        raw.endedAt = event.time;
        const reason = data.reason as { kind?: string; error?: { message?: string } } | undefined;
        switch (reason?.kind) {
          case 'completed':
            raw.status = 'completed';
            break;
          case 'aborted':
          case 'interrupted':
            raw.status = 'interrupted';
            raw.stoppedAt = clock();
            for (const tool of raw.tools) {
              if (tool.state === 'running') tool.state = 'stopped';
            }
            if (raw.question && raw.question.status === 'pending') {
              raw.question = { status: 'void', questions: raw.question.questions };
            }
            break;
          case 'error': {
            raw.status = 'failed';
            const failure = reason.error as { message?: string; code?: string } | undefined;
            const connectionName = raw.modelId
              ? useKindStore.getState().connectionNames[raw.modelId]
              : undefined;
            raw.failure = {
              cause: event.cause ?? 'model',
              reason: modelErrorReason(failure?.code, connectionName),
              code: failure?.code,
            };
            break;
          }
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
        const connectionName = raw.modelId
          ? useKindStore.getState().connectionNames[raw.modelId]
          : undefined;
        raw.retry = {
          attempt: typeof data.retry === 'number' ? data.retry : 1,
          reason: retryReason(failure.code, connectionName),
          inSeconds: Math.max(1, Math.round(delayMs / 1000)),
          code: failure.code,
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
        const view = compactionViewOf(data, auto);
        const window = useKindStore.getState().modelWindows[String(data.model ?? '')];
        view.before = contextPercent(contextTokens(data.usage), window) ?? undefined;
        raw.compaction = view;
        pendingCompaction.set(turn, { view, window });
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
      startedAt: raw.startedAt === null ? null : Math.floor(raw.startedAt / 1000),
      durationMs:
        raw.startedAt === null || raw.endedAt === null
          ? null
          : Math.max(0, raw.endedAt - raw.startedAt),
      status: raw.status,
      user: raw.user,
      reasoning: null,
      tools: raw.tools,
      cards: [],
      answer: raw.answer ? { text: raw.answer, elements: [] } : null,
      thinkingSeconds: thinkingSecondsOf(raw.reasoningMs),
      tokens: {
        input: raw.usage.input,
        output: raw.usage.output,
        cacheRead: raw.usage.cacheRead,
        cacheWrite: raw.usage.cacheWrite,
      },
      model: modelLabel(raw.modelId, useKindStore.getState().modelNames),
      cost: null,
      retry: raw.retry,
      compaction: raw.compaction,
      failure: raw.failure,
      question: raw.question,
      delivery: null,
      stoppedAt: raw.stoppedAt,
    }));
}

/** 改一轮：按 id 换成新对象（TurnRow 用 memo 比引用，只有这一条重绘）。 */


/** T013.5：把一轮的工具/思考折成工作过程的步骤（页面用；cwd 用于行上的路径相对化）。 */
export function processStepsOf(view: TurnViewData, cwd: string): ProcessStep[] {
  void cwd;
  const steps: ProcessStep[] = [];
  if (view.reasoning?.full) {
    steps.push({
      kind: 'think',
      id: view.id + '-think',
      text: view.reasoning.full,
      streaming: view.status === 'inProgress' && view.tools.some((tool) => tool.state === 'running' && tool.id === view.tools[view.tools.length - 1]?.id) === false && view.tools.length === 0,
    });
  }
  for (const tool of view.tools) {
    steps.push({
      kind: 'tool',
      id: tool.id,
      name: tool.name,
      args: tool.args,
      output: tool.output,
      exitCode: tool.exitCode,
      signal: tool.signal ?? null,
      durationMs: tool.durationMs,
      state: tool.state,
      meta: tool.meta ?? null,
    });
  }
  return steps;
}
