/// T009.7：Kind 的轮次视图与从日志事件派生（只搬代码，行为不变）。

import type { SendAttachmentRef, ServiceEvent, AskAnswer, FailureCause } from '@/data/service';
import type { AskQuestion } from '@/data/questionView';
import type { TurnStatus } from '@/data/types';
import { modelErrorReason, retryReason } from '../../data/errors.ts';
import { parseExitStatus } from './process/toolModel.ts';
import type { ProcessStep } from './process/toolModel.ts';
import { reasoningTextOf, reasoningTextOfStream } from './reasoning.ts';
import type { ConfirmCardFact, ConfirmDecision } from './cards/model';
import { dispatchedTaskIdOf, parseRestartNotice, parseTaskRelay, taskDispatchFactOf } from '../../data/taskResult.ts';
import { foldedTaskCard } from './taskCardFold.ts';
import { contextPercent, contextTokens, emptyUsage, mergeUsage, modelLabel, reasoningSpanMs, thinkingSeconds as thinkingSecondsOf } from '../../data/turnUsage.ts';

import type { CanvasViewDoc } from './canvas.ts';
import { gateVerdictOf, upstreamChangedOf, userRpcIdOf } from './relay.ts';
import { isProgramRpc } from '../../data/programMessages.ts';
import type { CompactionView, InterimSay, MemoCardInfo, ToolStep, TurnLabels, TurnUserView, TurnViewData } from './viewTypes.ts';

import type { QuestionView } from './viewTypes.ts';
import { canvasSourceOf, canvasVersionOf } from './canvas.ts';
import { taskCcOfMessage } from './relay.ts';
import type { TaskResultFact } from '../../data/taskResult.ts';
import type { UsageTotals } from '../../data/turnUsage.ts';

export * from './canvas.ts';
export * from './relay.ts';
export * from './viewTypes.ts';

function maskedReason(message: string | undefined): string {
  if (!message) return modelErrorReason('MODEL_DISABLED');
  const marker = 'MODEL_DISABLED: ';
  const at = message.indexOf(marker);
  return at >= 0 ? message.slice(at + marker.length) : message;
}




/**
 * 往上翻时把更早的一页接在前面：同一行（id）已经在列表里的不再加一遍。
 *
 * 抄送有两种日志事实（inbox/spliced 和领取后的 user/message），恰好分在两页时同一条
 * request id 会各成一行；按 id 去重后跨分页仍然只画一行。
 */
export function prependOlderViews(
  older: TurnViewData[],
  current: TurnViewData[],
): TurnViewData[] {
  if (older.length === 0) return current;
  const existing = new Set(current.map((view) => view.id));
  const fresh = older.filter((view) => !existing.has(view.id));
  return fresh.length === 0 ? current : fresh.concat(current);
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

export interface ManualSummaryMark {
  manual: 'from' | 'upTo';
  /** 总结到这里为止选中的那句 seq（旧日志没有就是 undefined）。 */
  targetSeq?: number;
}

export function compactionViewOf(
  data: Record<string, unknown>,
  auto: boolean,
  manual?: ManualSummaryMark,
): CompactionView {
  return {
    auto,
    ...(manual ? { manual: manual.manual } : {}),
    ...(manual?.targetSeq !== undefined ? { targetSeq: manual.targetSeq } : {}),
    summary: summaryTextOf(data),
  };
}

/** 两种总结的 sourceCommandId 标记（agent rewind_summarize_* 写入，
 * 形如 rewind:from:<seq> / rewind:up-to:<seq>；旧标记没有 seq 也认）。 */
export function manualSummaryOf(sourceCommandId: unknown): ManualSummaryMark | undefined {
  if (typeof sourceCommandId !== 'string') return undefined;
  const match = /^rewind:(from|up-to)(?::(\d+))?$/.exec(sourceCommandId);
  if (!match) return undefined;
  const parsed = match[2] === undefined ? undefined : Number(match[2]);
  return {
    manual: match[1] === 'from' ? 'from' : 'upTo',
    ...(parsed !== undefined && Number.isFinite(parsed) ? { targetSeq: parsed } : {}),
  };
}

/** 总结分隔的伪一轮（不属于任何 turn；刷新/翻页后也能重建）。 */
function dividerViewOf(
  id: string,
  compaction: CompactionView,
  seqStart?: number,
): TurnViewData {
  return {
    id,
    ...(seqStart !== undefined ? { seqStart } : {}),
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
}

/** 把一条总结分隔插回它在轮次之间的位置：到这里为止插在选中那句前，其余按事件 seq。 */
function insertDivider(views: TurnViewData[], divider: TurnViewData): void {
  const target = divider.compaction?.targetSeq;
  if (divider.compaction?.manual === 'upTo' && target !== undefined) {
    const index = views.findIndex((view) => view.user?.seq === target);
    if (index >= 0) {
      views.splice(index, 0, divider);
      return;
    }
  }
  if (divider.seqStart !== undefined) {
    const index = views.findIndex(
      (view) => typeof view.seqStart === 'number' && view.seqStart > divider.seqStart!,
    );
    if (index >= 0) {
      views.splice(index, 0, divider);
      return;
    }
  }
  views.push(divider);
}

/** 刷新/翻页/实时追加后，把总结分隔按 targetSeq/seq 重新放回轮次之间。 */
export function placeManualDividers(views: TurnViewData[]): TurnViewData[] {
  const dividers = views.filter((view) => view.compaction?.manual);
  if (dividers.length === 0) return views;
  const others = views.filter((view) => !view.compaction?.manual);
  for (const divider of dividers) insertDivider(others, divider);
  return others;
}

export function deriveTurnViews(
  events: ServiceEvent[],
  labels: TurnLabels = {},
): TurnViewData[] {
  interface Raw {
    turn: number;
    /** 这一轮第一条日志事件的 seq（放总结分隔时定位）。 */
    seqStart?: number;
    startedAt: number | null;
    endedAt: number | null;
    user: TurnUserView | null;
    taskResults: TaskResultFact[];
    says: InterimSay[];
    answer: string | null;
    sayParts: string[];
    reaction: string | null;
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
  // T053：门判定 relay（gate-verdict）按被把关任务 id 回填结果卡状态词，刷新可重建。
  const gateVerdicts = new Map<
    string,
    { conclusion: 'pass' | 'needs_fix'; candidateReport: string }
  >();
  // T053：上游变了的下游（upstream_changed relay；该任务交出新 ready 报告时清除）。
  const upstreamChangedTasks = new Set<string>();
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
      at: event.time / 1000,
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
  /** 没有 turn 的 compaction/summary（手动压缩、两种总结）先收起来，等轮建完再插。 */
  const orphanDividers: { seq: number; view: CompactionView }[] = [];
  let currentSeq = 0;
  const ensure = (turn: number): Raw => {
    let raw = turns.get(turn);
    if (!raw) {
      raw = {
        turn,
        seqStart: currentSeq,
        startedAt: null,
        endedAt: null,
        user: null,
        taskResults: [],
        says: [],
        answer: null,
        sayParts: [],
        reaction: null,
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
  // T038：派发卡。task_accept 的 tool/call 派生一张「派出中→进行中」的卡；tool/result 里
  // 的「已派出任务 <id>」补上 task id；之后的 relay（task-report/task-stop/tasks_interrupted）
  // 按 task id 归并到同一张卡上更新状态，不另起第二张（跨轮折叠照画布的先例）。
  const dispatchCards = new Map<string, { turn: number; index: number }>();
  const dispatchByTaskId = new Map<string, string>();
  const mergeRelayIntoDispatch = (fact: TaskResultFact): boolean => {
    const callId = dispatchByTaskId.get(fact.taskId);
    const position = callId === undefined ? undefined : dispatchCards.get(callId);
    const raw = position === undefined ? undefined : turns.get(position.turn);
    const existing = raw && position ? raw.taskResults[position.index] : undefined;
    if (!raw || !position || !existing) return false;
    // 只把「派发后第一条 relay」并回派发卡；判定和跨轮折卡共用 foldedTaskCard，别分叉。
    const merged = foldedTaskCard(existing, fact);
    if (!merged) return false;
    // 同一张卡更新成 relay 的事实（状态、摘要、问题、写入范围、回填接受决定都用 relay 的 id）。
    raw.taskResults[position.index] = merged;
    return true;
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
  const userOf = (message: unknown, rpcId: string | null): TurnUserView | null => {
    if (!message || typeof message !== 'object') return null;
    const content = (message as { content?: unknown }).content;
    if (!Array.isArray(content)) return null;
    const text: string[] = [];
    const pastes: string[] = [];
    const files: SendAttachmentRef[] = [];
    const images: SendAttachmentRef[] = [];
    const objectRefs: TurnUserView['objectRefs'] = [];
    const quotes: TurnUserView['quotes'] = [];
    let canvas: { id: string; title: string } | null = null;
    let resultCard: { taskId: string; title: string; version: number } | null = null;
    for (const block of content) {
      if (!block || typeof block !== 'object') continue;
      const typed = block as {
        type?: string;
        text?: unknown;
        paste?: unknown;
        canvas?: unknown;
        pageContext?: unknown;
        id?: unknown;
        attachmentId?: unknown;
        name?: unknown;
        bytes?: unknown;
        taskId?: unknown;
        version?: unknown;
        title?: unknown;
        kind?: unknown;
        source?: unknown;
      };
      // T054.2：结果卡块（用户点「要求修改」带进输入框的那张卡）。
      if (
        typed.type === 'result-card' &&
        typeof typed.taskId === 'string' &&
        typed.taskId !== '' &&
        typeof typed.title === 'string'
      ) {
        resultCard = {
          taskId: typed.taskId,
          title: typed.title,
          version: typeof typed.version === 'number' ? typed.version : 0,
        };
        continue;
      }
      if (typed.type === 'text') {
        const value = typeof typed.text === 'string' ? typed.text : '';
        // T015.4：画布放进来的那一句不进正文——气泡上画成「来自画布」标记，正文只留打的字。
        const source = canvasSourceOf(typed.canvas);
        if (source) {
          canvas = source;
          continue;
        }
        // 页面 Kind 的程序来源只给模型看；用户气泡只留自己打的字。
        if (typed.pageContext && typeof typed.pageContext === 'object') continue;
        if (typed.paste === true) pastes.push(value);
        else text.push(value);
        continue;
      }
      if (typed.type === 'quote' && typeof typed.text === 'string') {
        const source = typed.source as
          | {
              kind?: unknown;
              id?: unknown;
              title?: unknown;
              version?: unknown;
              taskId?: unknown;
            }
          | undefined;
        quotes.push({
          text: typed.text,
          source:
            source && typeof source.id === 'string'
              ? {
                  kind: typeof source.kind === 'string' ? source.kind : 'task',
                  id: source.id,
                  title: typeof source.title === 'string' ? source.title : '',
                  version: typeof source.version === 'number' ? source.version : undefined,
                  taskId: typeof source.taskId === 'string' ? source.taskId : undefined,
                }
              : null,
        });
        continue;
      }
      if (typed.type === 'object-ref' && typeof typed.id === 'string') {
        objectRefs.push({
          kind: typeof typed.kind === 'string' ? typed.kind : 'task',
          id: typed.id,
          title: typeof typed.title === 'string' ? typed.title : '',
          version: typeof typed.version === 'number' ? typed.version : undefined,
          taskId: typeof typed.taskId === 'string' ? typed.taskId : undefined,
        });
        continue;
      }
      if (typed.type === 'image' && typeof typed.attachmentId === 'string') {
        images.push({
          id: typed.attachmentId,
          name: typeof typed.name === 'string' ? typed.name : '',
          bytes: typeof typed.bytes === 'number' ? typed.bytes : 0,
          sentAs: typeof (typed as { sentAs?: unknown }).sentAs === 'string' ? (typed as { sentAs: string }).sentAs : undefined,
        });
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
    if (
      !body &&
      pastes.length === 0 &&
      files.length === 0 &&
      images.length === 0 &&
      objectRefs.length === 0 &&
      quotes.length === 0 &&
      !canvas &&
      !resultCard
    )
      return null;
    return { text: body, elements: [], pastes, files, images, objectRefs, quotes, canvas, resultCard, rpcId };
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
    currentSeq = event.seq;
    const data = event.data ?? {};
    if (event.type === 'assistant/message') {
      const id = (data.message as { id?: unknown } | undefined)?.id;
      if (typeof id === 'string') {
        if (seenAssistantMessages.has(id)) continue;
        seenAssistantMessages.add(id);
      }
    }
    const turn = typeof data.turn === 'number' ? data.turn : null;
    if (turn === null) {
      // 没有 turn 的 compaction/summary（手动压缩、两种总结）也要出分隔；
      // 先收起来，等轮次建完按 seq/targetSeq 插回原位。
      if (event.type === 'compaction/summary') {
        const manual = manualSummaryOf(data.sourceCommandId);
        const auto = manual
          ? false
          : data.sourceCommandId === undefined || data.sourceCommandId === null;
        orphanDividers.push({ seq: event.seq, view: compactionViewOf(data, auto, manual) });
      }
      continue;
    }
    switch (event.type) {
      case 'turn/start': {
        const raw = ensure(turn);
        raw.startedAt = event.time;
        break;
      }
      case 'user/message': {
        // 启动扫描的重启汇报：一张卡一个被打断的任务，代替程序消息的原文 JSON。
        const restarted = parseRestartNotice(data.message, event.time / 1000);
        if (restarted.length > 0) {
          // T038：派出过的任务把被打断的状态并回那张派发卡；找不到派发卡的照旧各画一张。
          for (const fact of restarted) {
            if (!mergeRelayIntoDispatch(fact)) ensure(turn).taskResults.push(fact);
          }
          break;
        }
        // T053：上游有新结果的 relay 只给模型看，不画用户气泡；给下游卡标「待按新结果核对」。
        if (userRpcIdOf(data.message)?.startsWith('upstream-changed:')) {
          const changed = upstreamChangedOf(data.message);
          if (changed) upstreamChangedTasks.add(changed.downstreamId);
          break;
        }
        const relay = parseTaskRelay(data.message, event.time / 1000, event.seq);
        if (relay) {
          // 下游交出新 ready 报告：需重评标记清除。
          upstreamChangedTasks.delete(relay.taskId);
          // 同一轮可能被多条 relay 叫起来：按日志到达顺序各留一张卡，不互相覆盖。
          // T038：派出过的任务并回那张派发卡（一张任务只渲染一张卡）。
          if (!mergeRelayIntoDispatch(relay)) ensure(turn).taskResults.push(relay);
          break;
        }
        // 固定块补发等程序消息：只进模型请求，不画成用户气泡。
        // 坏 JSON 的 task-report 解析不出卡，退回原始文本气泡，不按程序消息吞掉。
        const rpcId = userRpcIdOf(data.message);
        if (
          isProgramRpc(rpcId) &&
          !rpcId?.startsWith('task-report:') &&
          !rpcId?.startsWith('appointment-wake:')
        ) {
          break;
        }
        // T028.5：抄送 Kind 的那条不是用户气泡，单独由 taskCcFacts/ccViewOf 成行。
        if (taskCcOfMessage(data.message)) break;
        // T053：门判定的 relay 只给模型看结论，不把原文 JSON 画成用户气泡；
        // 结论回填被把关任务卡的状态词（任务要求已满足 / 需修正）。
        if (userRpcIdOf(data.message)?.startsWith('gate-verdict:')) {
          const relay = gateVerdictOf(data.message);
          if (relay) {
            gateVerdicts.set(relay.taskId, {
              conclusion: relay.conclusion,
              candidateReport: relay.candidateReport,
            });
          }
          break;
        }
        const source = data.message as
          | { source?: string | { kind?: string } }
          | undefined;
        const kind =
          typeof source?.source === 'string' ? source.source : source?.source?.kind;
        if (kind !== undefined && kind !== 'user' && kind !== 'skill-invocation') break;
        const view = userOf(data.message, userRpcIdOf(data.message));
        if (view) {
          view.seq = event.seq;
          ensure(turn).user = view;
        }
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
        if (text && preTool) {
          // D89：工具前说明按日志先后进工作过程，不进回答区。
          if (text.trim()) {
            raw.says.push({ id: 'say-' + event.seq, text, seq: event.seq });
          }
        } else if (text) {
          raw.answer = text;
        }
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
        const callId = String(data.call_id ?? data.callId ?? '');
        raw.tools.push({
          id: callId,
          name: String(data.name ?? ''),
          args,
          output: '',
          exitCode: null,
          durationMs: null,
          state: 'running',
          startedAtMs: event.time,
          seq: event.seq,
        });
        // T038：Kind 的 task_accept 一发起就先出一张派发卡（这时还没有 task id：派出中）。
        if (String(data.name ?? '') === 'task_accept') {
          const dispatch = taskDispatchFactOf(callId, args, event.time / 1000);
          if (dispatch) {
            dispatchCards.set(callId, { turn, index: raw.taskResults.length });
            raw.taskResults.push(dispatch);
          }
        }
        break;
      }
      case 'tool/result': {
        const raw = ensure(turn);
        // T013.5：按 call id 配对（不再用「最后一条 running」猜），并留下 meta。
        const message = (data.message ?? {}) as Record<string, unknown>;
        const callId = String(message.toolCallId ?? message.tool_call_id ?? '');
        // 问题卡等回答时用户直接发送：agent 在工具结果 meta 写结构化标记（N3），
        // 页面只从 meta 派生 replied，不再匹配工具结果正文（文案改了回执也不丢）。
        const resultMeta = data.meta as { ask_direct_reply?: unknown } | undefined;
        if (
          callId &&
          raw.question &&
          (raw.question.status === 'dismissed' || raw.question.status === 'pending') &&
          raw.question.batchId === callId
        ) {
          if (resultMeta?.ask_direct_reply === true) {
            raw.question = {
              status: 'replied',
              batchId: raw.question.batchId,
              questions: raw.question.questions,
            };
          } else if (resultMeta === undefined) {
            // 旧日志没有 meta——不判答没答；收成一行显示工具结果原文，不回填日志。
            const text = (textOfToolResult(data.message) || textOf(data.message)).trim();
            if (text) {
              raw.question = {
                status: 'legacy-reply',
                batchId: raw.question.batchId,
                questions: raw.question.questions,
                text,
              };
            }
          }
        }
        const step = callId ? raw.tools.find((item) => item.id === callId) : undefined;
        if (step) {
          // 普通工具错误没有结构化 error，消息级的 isError 同样是失败。
          const failed =
            Boolean(data.error) || message.isError === true || message.is_error === true;
          step.state = failed ? 'error' : 'done';
          const body = textOf(data.message) || textOfToolResult(data.message);
          step.output = body || String((data.error as { code?: string } | undefined)?.code ?? '');
          step.meta = (data.meta as Record<string, unknown> | undefined) ?? null;
          if (step.name === 'kind_react' && !failed) {
            const reaction = step.meta?.kind_react as { emoji_id?: unknown } | undefined;
            if (typeof reaction?.emoji_id === 'string') raw.reaction = reaction.emoji_id;
          }
          if (step.name === 'kind_say' && !failed) {
            const say = step.meta?.kind_say as { parts?: unknown } | undefined;
            if (Array.isArray(say?.parts)) {
              raw.sayParts = say.parts.filter(
                (part): part is string => typeof part === 'string' && part.trim() !== '',
              );
            }
          }
          // T015.1：画布全文只从日志的 meta 来；回放和实时同一条路。
          raw.canvas = canvasOf(step.meta) ?? raw.canvas;
          const status = parseExitStatus(step.output);
          step.exitCode = status.exitCode;
          step.signal = status.signal;
          if (step.startedAtMs) step.durationMs = Math.max(0, event.time - step.startedAtMs);
        }
        // T038：task_accept 的回执带任务 id → 派发卡从「派出中」变「进行中」，并记下 id 供 relay 归并。
        const dispatch = callId ? dispatchCards.get(callId) : undefined;
        if (dispatch) {
          const body = textOf(data.message) || textOfToolResult(data.message);
          const taskId = dispatchedTaskIdOf(body);
          const fact = turns.get(dispatch.turn)?.taskResults[dispatch.index];
          if (fact && taskId) {
            fact.taskId = taskId;
            fact.state = 'dispatched';
            dispatchByTaskId.set(taskId, callId);
          }
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
              ? labels.connectionNames?.[raw.modelId]
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
          ? labels.connectionNames?.[raw.modelId]
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
        const manual = manualSummaryOf(data.sourceCommandId);
        const auto = manual
          ? false
          : data.sourceCommandId === undefined || data.sourceCommandId === null;
        const view = compactionViewOf(data, auto, manual);
        const window = labels.modelWindows?.[String(data.model ?? '')];
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
  // 结果卡的接受决定：card/decision 的 callId 就是 relay 的 request id；刷新后从日志重推。
  for (const raw of turns.values()) {
    for (const fact of raw.taskResults) {
      // 门判定只作用在它判的那份报告上；下游交了更新结果后不再套旧结论（显示等核查）。
      const gateVerdict = gateVerdicts.get(fact.taskId);
      if (gateVerdict && fact.reportId === gateVerdict.candidateReport) {
        fact.gateVerdict = gateVerdict.conclusion;
      }
      if (upstreamChangedTasks.has(fact.taskId)) fact.upstreamChanged = true;
      const decision = cardDecisions.get(fact.id);
      if (decision) {
        fact.decision = {
          phase: decision.phase,
          ...(decision.data === undefined ? {} : { data: decision.data }),
          ...(decision.at === undefined ? {} : { at: decision.at }),
        };
      }
    }
  }
  const views: TurnViewData[] = [...turns.values()]
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
      taskResults: raw.taskResults,
      says: raw.says,
      reasoning:
        raw.reasoningParts.length > 0 ? { full: raw.reasoningParts.join('\n\n') } : null,
      tools: raw.tools,
      answer: raw.answer ? { text: raw.answer, elements: [] } : null,
      sayParts: raw.sayParts,
      reaction: raw.reaction,
      thinkingSeconds: thinkingSecondsOf(raw.reasoningMs),
      tokens: {
        input: raw.usage.input,
        output: raw.usage.output,
        cacheRead: raw.usage.cacheRead,
        cacheWrite: raw.usage.cacheWrite,
      },
      model: modelLabel(raw.modelId, labels.modelNames ?? {}),
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
  for (const item of orphanDividers) {
    const id = 'turn-' + String(item.seq + 0.5) + '-compaction-' + String(item.seq);
    insertDivider(views, dividerViewOf(id, item.view, item.seq));
  }
  const memoCards: MemoCardInfo[] = events
    .filter((event) => event.type === 'memory/card')
    .map((event) => {
      const data = (event.data ?? {}) as { id?: unknown; kind?: unknown; ref?: unknown };
      return {
        id: typeof data.id === 'string' ? data.id : '',
        kind: typeof data.kind === 'string' ? data.kind : '',
        ref: typeof data.ref === 'string' ? data.ref : undefined,
      };
    })
    .filter((card) => card.id !== '' && card.kind !== '');
  if (memoCards.length > 0 && views.length > 0) {
    views[views.length - 1] = { ...views[views.length - 1], memoCards };
  }
  return views;
}

/** 改一轮：按 id 换成新对象（TurnRow 用 memo 比引用，只有这一条重绘）。 */

// T038：跨轮/跨页折卡在 taskCardFold.ts（纯函数，node 用例直接跑这份）。
export { reconcileTaskCards } from './taskCardFold.ts';


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
  // D89：工具前说明和工具步按日志 seq 排先后；没有 seq 的旧数据排在最后，
  // 保持工具之间的原顺序。
  const ordered: { seq: number; step: ProcessStep }[] = [
    ...view.tools
      .filter((tool) => tool.name !== 'kind_say' && tool.name !== 'kind_react')
      .map((tool) => ({
      seq: tool.seq ?? Number.MAX_SAFE_INTEGER,
      step: {
        kind: 'tool' as const,
        id: tool.id,
        name: tool.name,
        args: tool.args,
        output: tool.output,
        exitCode: tool.exitCode,
        signal: tool.signal ?? null,
        durationMs: tool.durationMs,
        state: tool.state,
        meta: tool.meta ?? null,
      },
    })),
    ...view.says.map((say) => ({
      seq: say.seq,
      step: { kind: 'say' as const, id: say.id, text: say.text },
    })),
  ];
  ordered.sort((left, right) => left.seq - right.seq);
  for (const item of ordered) steps.push(item.step);
  return steps;
}
