/// T009.7：Kind 的轮次视图与从日志事件派生（只搬代码，行为不变）。

import type { SendAttachmentRef, ServiceEvent } from '@/data/service';
import type { AskAnswer, FailureCause } from '@/data/service';
import type { AskQuestion } from '@/data/questionView';
import type { TextElement, TurnStatus } from '@/data/types';
import { modelErrorReason, retryReason } from '@/data/errors';

/** 被屏蔽的失败说明：失败块显示内核给的原文（模型名 + 命中的规则），去掉错误包装的前缀。 */
function maskedReason(message: string | undefined): string {
  if (!message) return modelErrorReason('MODEL_DISABLED');
  const marker = 'MODEL_DISABLED: ';
  const at = message.indexOf(marker);
  return at >= 0 ? message.slice(at + marker.length) : message;
}
import { parseExitStatus } from './process/toolModel';
import type { ProcessStep } from './process/toolModel';
import { reasoningTextOf, reasoningTextOfStream } from './reasoning';
import { useKindStore } from './kindStore';
import type { ConfirmCardFact, ConfirmDecision } from './cards/model';
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

/**
 * 模型提问：等你答 → 答了 / 不答 / 这一轮被停了。
 * T017.2：带问题批次 id（ask_user_question 调用的 call id），卡片信封的 id 用它，不用数组下标。
 */
export type QuestionView =
  | { status: 'pending'; batchId: string; questions: AskQuestion[] }
  | { status: 'answered'; batchId: string; questions: AskQuestion[]; answers: AskAnswer[] }
  | { status: 'dismissed'; batchId: string; questions: AskQuestion[] }
  | { status: 'void'; batchId: string; questions: AskQuestion[] };

/**
 * 画布（T015.1）：日志里 tool/result.meta.canvas 的那一版全文。
 * 回放和实时都走同一份事件，所以这里只有日志事实，没有页面自己的状态。
 */
export interface CanvasViewDoc {
  id: string;
  /** 第几版（T015.1 只出第 1 版；版本切换在 T015.2）。 */
  n: number;
  title: string;
  note: string;
  html: string;
}

/** 画布的一个版本（日志里的一条 tool/result.meta.canvas）。 */
export interface CanvasVersionView {
  /** 第几版，从 1 开始。 */
  n: number;
  title: string;
  note: string;
  html: string;
}

/** 一个画布出过的版本（按 n 升序）和最新版在哪一轮。 */
export interface CanvasTrack {
  latest: number;
  /** 最新一版所在轮次的视图 id（旧版缩成一行时点它跳过去）。 */
  latestTurnId: string;
  versions: CanvasVersionView[];
}

/** 从日志折叠每个画布的版本（T015.2）：只认 tool/result.meta.canvas。 */
export function canvasTracks(events: ServiceEvent[]): Record<string, CanvasTrack> {
  return mergeCanvasTracks({}, events);
}

/** 把一批事件折进已有的版本表（首屏、往上翻、live 三处共用）。 */
export function mergeCanvasTracks(
  tracks: Record<string, CanvasTrack>,
  events: ServiceEvent[],
): Record<string, CanvasTrack> {
  let next = tracks;
  for (const event of events) {
    if (event.type !== 'tool/result') continue;
    const turn = typeof event.data.turn === 'number' ? event.data.turn : null;
    if (turn === null) continue;
    const version = canvasVersionOf(event.data.meta);
    if (!version) continue;
    const track = next[version.id] ?? { latest: 0, latestTurnId: '', versions: [] };
    const versions = track.versions
      .filter((item) => item.n !== version.n)
      .concat({ n: version.n, title: version.title, note: version.note, html: version.html })
      .sort((left, right) => left.n - right.n);
    next = {
      ...next,
      [version.id]: {
        latest: Math.max(track.latest, version.n),
        // 同一版重复到达时认后一次（live 重放同一条事件不该丢轮次）。
        latestTurnId: version.n >= track.latest ? 'turn-' + String(turn) : track.latestTurnId,
        versions,
      },
    };
  }
  return next;
}

/** T015.4：user/message 文字块上的 canvas（{id, title} 都是字符串才算；形状不对就当普通文字）。 */
function canvasSourceOf(value: unknown): { id: string; title: string } | null {
  if (!value || typeof value !== 'object') return null;
  const typed = value as { id?: unknown; title?: unknown };
  if (typeof typed.id !== 'string' || typeof typed.title !== 'string') return null;
  return { id: typed.id, title: typed.title };
}

/** tool/result.meta.canvas → 一版画布（形状不对就当没有）。 */
function canvasVersionOf(meta: unknown): (CanvasVersionView & { id: string }) | null {
  if (!meta || typeof meta !== 'object') return null;
  const value = (meta as { canvas?: unknown }).canvas;
  if (!value || typeof value !== 'object') return null;
  const typed = value as { id?: unknown; n?: unknown; title?: unknown; note?: unknown; html?: unknown };
  if (typeof typed.id !== 'string' || typeof typed.title !== 'string' || typeof typed.html !== 'string') {
    return null;
  }
  return {
    id: typed.id,
    n: typeof typed.n === 'number' ? typed.n : 1,
    title: typed.title,
    note: typeof typed.note === 'string' ? typed.note : '',
    html: typed.html,
  };
}

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
  /** T015.4：这句话是画布里的按钮放进来的（日志块上的 canvas：id + 标题）；没有就是自己写的。 */
  canvas: { id: string; title: string } | null;
}

export interface TurnViewData {
  id: string;
  startedAt: number | null;
  durationMs: number | null;
  status: TurnStatus;
  user: TurnUserView | null;
  /** 思考全文（T013 验收 F5）；摘要和秒数页面不用，日志里也没有（T013 C1）。 */
  reasoning: { full: string } | null;
  tools: ToolStep[];
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
  /** 这一轮出的画布（日志里 tool/result.meta.canvas）。 */
  canvas: CanvasViewDoc | null;
  /** 这一轮回答下面的确认卡（日志里 propose_action 的提议 + card/decision）。 */
  card: ConfirmCardFact | null;
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
    /** T013 验收 F5：这一轮各次模型尝试的 reasoning 全文（按出现顺序）。 */
    reasoningParts: string[];
    failure: { cause: FailureCause; reason: string; code?: string } | null;
    question: QuestionView | null;
    retry: TurnViewData['retry'];
    compaction: CompactionView | null;
    canvas: CanvasViewDoc | null;
    card: ConfirmCardFact | null;
    answered: AskAnswer[];
    cancelled: number;
    stoppedAt: string | null;
  }
  const turns = new Map<number, Raw>();
  // 确认卡：提议是 propose_action 的 tool/call + 成功结果（结果 meta.card），决定是
  // card/decision 事件；replaced 由「后面还有一条被接受的新卡」推导，不单独记。
  interface CardEntry {
    turn: number;
    seq: number;
    proposal: unknown;
  }
  const cardEntries = new Map<string, CardEntry>();
  const cardDecisions = new Map<string, ConfirmDecision>();
  for (const event of events) {
    const data = event.data ?? {};
    if (event.type === 'tool/call' && data.name === 'propose_action') {
      const callId = String(data.callId ?? data.call_id ?? '');
      const turn = typeof data.turn === 'number' ? data.turn : null;
      if (callId && turn !== null) {
        cardEntries.set(callId, { turn, seq: event.seq, proposal: null });
      }
      continue;
    }
    if (event.type === 'tool/result') {
      const message = (data.message ?? {}) as Record<string, unknown>;
      const callId = String(message.toolCallId ?? message.tool_call_id ?? '');
      const entry = cardEntries.get(callId);
      const meta = data.meta as { card?: unknown } | undefined;
      if (!entry || !meta?.card || data.error) continue;
      entry.proposal = meta.card;
      entry.seq = event.seq;
      continue;
    }
    if (event.type !== 'card/decision') continue;
    const callId = String(data.callId ?? data.call_id ?? '');
    const phase = data.phase;
    if (
      !callId ||
      (phase !== 'confirmed' && phase !== 'dismissed' && phase !== 'refused' && phase !== 'failed')
    ) {
      continue;
    }
    const payload = data.data && typeof data.data === 'object' ? data.data : undefined;
    cardDecisions.set(callId, {
      phase,
      option: typeof data.option === 'string' ? data.option : undefined,
      action: typeof data.action === 'string' ? data.action : undefined,
      data: payload as ConfirmDecision['data'],
    });
  }
  let latestCardSeq = 0;
  for (const entry of cardEntries.values()) {
    if (entry.proposal) latestCardSeq = Math.max(latestCardSeq, entry.seq);
  }
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
        reasoningParts: [],
        failure: null,
        question: null,
        retry: null,
        compaction: null,
        canvas: null,
        card: null,
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
   * T023 补修：assistant/message 的正文里有没有工具调用块。
   * 带工具调用的正文是这一步的工具前说明，不是这一轮的最终回答。
   */
  const hasToolCall = (message: unknown): boolean => {
    if (!message || typeof message !== 'object') return false;
    const content = (message as { content?: unknown }).content;
    if (!Array.isArray(content)) return false;
    return content.some(
      (block) =>
        block !== null &&
        typeof block === 'object' &&
        (block as { type?: string }).type === 'tool-call',
    );
  };

  /**
   * T023 补修：这次尝试的流记录里有没有工具调用块。
   * 取消收尾时内核会丢掉 tool-call 块、只留 text 并带 interrupted=true，所以
   * 「这段正文是不是工具前说明」要看流记录；只看消息内容会把被屏蔽打断的半截
   * 回答也一起丢掉。
   */
  const streamHasToolCall = (stream: unknown): boolean => {
    if (!Array.isArray(stream)) return false;
    return stream.some((record) => {
      if (!record || typeof record !== 'object') return false;
      const typed = record as {
        type?: string;
        chunk?: { type?: string; blockType?: string; block?: { type?: string } };
      };
      if (typed.type === 'tool-call-chunks') return true;
      if (!typed.chunk) return false;
      if (typed.chunk.type === 'block-start') return typed.chunk.blockType === 'tool-call';
      if (typed.chunk.type === 'block-end') return typed.chunk.block?.type === 'tool-call';
      return false;
    });
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
    let canvas: { id: string; title: string } | null = null;
    for (const block of content) {
      if (!block || typeof block !== 'object') continue;
      const typed = block as {
        type?: string;
        text?: unknown;
        paste?: unknown;
        canvas?: unknown;
        attachmentId?: unknown;
        name?: unknown;
        bytes?: unknown;
      };
      if (typed.type === 'text') {
        const value = typeof typed.text === 'string' ? typed.text : '';
        // T015.4：画布放进来的那一句不进正文——气泡上画成「来自画布」标记，正文只留打的字。
        const source = canvasSourceOf(typed.canvas);
        if (source) {
          canvas = source;
          continue;
        }
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
    if (!body && pastes.length === 0 && files.length === 0 && !canvas) return null;
    return { text: body, elements: [], pastes, files, canvas };
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

  /** T015.1：tool/result.meta.canvas → 这一轮要画的画布（形状不对就当没有）。 */
  const canvasOf = (meta: Record<string, unknown> | null): CanvasViewDoc | null =>
    canvasVersionOf(meta);

  const modelIdOf = (message: unknown): string | null => {
    if (!message || typeof message !== 'object') return null;
    const source = (message as { source?: { kind?: string; model?: unknown } }).source;
    if (!source || source.kind !== 'model') return null;
    return typeof source.model === 'string' ? source.model : null;
  };

  /** T013 验收 F5：按出现顺序收 reasoning 全文；同一条尝试的 message/stream 不重复记。 */
  const pushReasoning = (raw: Raw, text: string) => {
    const value = text.trim();
    if (!value) return;
    if (raw.reasoningParts[raw.reasoningParts.length - 1] === value) return;
    raw.reasoningParts.push(value);
  };
  // T015.3：压缩剪枝替换的 assistant/message 与原文同一个消息 id（surface 替换视图）。
  // 页面看日志事实，只认第一次出现，免得旧轮状态被替换事件重置；旧版调用参数仍按原文显示。
  const seenAssistantMessages = new Set<string>();
  for (const event of events) {
    const data = event.data ?? {};
    if (event.type === 'assistant/message') {
      const id = (data.message as { id?: unknown } | undefined)?.id;
      if (typeof id === 'string') {
        if (seenAssistantMessages.has(id)) continue;
        seenAssistantMessages.add(id);
      }
    }
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
        // T023 补修：工具前说明不能进回答区，否则 turn/end 后回答区会回退显示它。
        // 正常完成时它和 tool-call 块在同一条消息里；取消收尾时内核摘掉 tool-call、
        // 只留 text 并带 interrupted=true，要从 stream 记录里认出这次尝试本来要调工具。
        // 只有 text 流的中断（被屏蔽打断的半截回答）照旧留在回答区。
        const preTool =
          hasToolCall(data.message) ||
          (data.interrupted === true && streamHasToolCall(data.stream));
        if (text && !preTool) raw.answer = text;
        raw.usage = mergeUsage(raw.usage, data.usage);
        raw.modelId = modelIdOf(data.message) ?? raw.modelId;
        raw.reasoningMs += reasoningSpanMs(data.stream);
        // 正文块里是思考全文；没有块时才退回 stream 的增量（失败/中断的那次尝试）。
        pushReasoning(raw, reasoningTextOf(data.message) || reasoningTextOfStream(data.stream));
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
        pushReasoning(raw, reasoningTextOfStream(data.stream));
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
          // 普通工具错误没有结构化 error，消息级的 isError 同样是失败。
          const failed =
            Boolean(data.error) || message.isError === true || message.is_error === true;
          step.state = failed ? 'error' : 'done';
          const body = textOf(data.message) || textOfToolResult(data.message);
          step.output = body || String((data.error as { code?: string } | undefined)?.code ?? '');
          step.meta = (data.meta as Record<string, unknown> | undefined) ?? null;
          // T015.1：画布全文只从日志的 meta 来；回放和实时同一条路。
          raw.canvas = canvasOf(step.meta) ?? raw.canvas;
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
          case 'interrupted': {
            // 屏蔽打断的一轮走失败块（和 MODEL_DISABLED 同一类），不写「你停下了这一轮」。
            const masked = event.cause === 'masked';
            raw.status = masked ? 'failed' : 'interrupted';
            raw.stoppedAt = clock();
            for (const tool of raw.tools) {
              if (tool.state === 'running') tool.state = 'stopped';
            }
            if (raw.question && raw.question.status === 'pending') {
              raw.question = {
                status: 'void',
                batchId: raw.question.batchId,
                questions: raw.question.questions,
              };
            }
            if (masked) {
              raw.failure = { cause: 'masked', reason: '模型在设置里被屏蔽了' };
            }
            break;
          }
          case 'error': {
            raw.status = 'failed';
            const failure = reason.error as { message?: string; code?: string } | undefined;
            const connectionName = raw.modelId
              ? useKindStore.getState().connectionNames[raw.modelId]
              : undefined;
            const cause = event.cause ?? 'model';
            raw.failure = {
              cause,
              // 被屏蔽的说明只有服务端知道（模型名 + 规则）；其余按错误码表写。
              reason:
                cause === 'masked'
                  ? maskedReason(failure?.message)
                  : modelErrorReason(failure?.code, connectionName),
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
        const batchId = String(data.call_id ?? data.callId ?? '');
        const current = raw.question;
        if (!current || current.status !== 'pending') {
          raw.question = { status: 'pending', batchId, questions: [asked] };
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
        const current = raw.question && raw.question.status !== 'dismissed' ? raw.question : null;
        const questions = current ? current.questions : [];
        const batchId = current?.batchId ?? String(data.call_id ?? data.callId ?? '');
        if (questions.length > 0 && raw.answered.length >= questions.length) {
          raw.question = { status: 'answered', batchId, questions, answers: raw.answered };
        } else if (questions.length > 0) {
          raw.question = { status: 'pending', batchId, questions };
        }
        break;
      }
      case 'question/cancelled': {
        const raw = ensure(turn);
        raw.cancelled += 1;
        const current = raw.question && raw.question.status !== 'answered' ? raw.question : null;
        const questions = current ? current.questions : [];
        const batchId = current?.batchId ?? String(data.call_id ?? data.callId ?? '');
        if (questions.length > 0 && raw.cancelled >= questions.length) {
          raw.question = { status: 'dismissed', batchId, questions };
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
  for (const [callId, entry] of cardEntries) {
    if (!entry.proposal) continue;
    const raw = turns.get(entry.turn);
    if (!raw) continue;
    const decision = cardDecisions.get(callId) ?? null;
    raw.card = {
      callId,
      proposal: entry.proposal,
      decision,
      replaced: decision === null && entry.seq < latestCardSeq,
    };
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
      reasoning:
        raw.reasoningParts.length > 0 ? { full: raw.reasoningParts.join('\n\n') } : null,
      tools: raw.tools,
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
      canvas: raw.canvas,
      card: raw.card,
      delivery: null,
      stoppedAt: raw.stoppedAt,
    }));
}

/** 改一轮：按 id 换成新对象（TurnRow 用 memo 比引用，只有这一条重绘）。 */


/** 把一轮的工具/思考折成工作过程的步骤（页面用；路径相对化在 process 那一层做）。 */
export function processStepsOf(view: TurnViewData): ProcessStep[] {
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
