//! cc/relay 辅助：抄送行、门判定、上游变了、用户 rpc 与 task relay 解析。

import type { ServiceEvent } from '@/data/service';
import type { TaskCcView, TurnViewData } from './viewTypes.ts';

/** ：抄送 Kind 的 request id 前缀（服务端 task-cc:<inputId>）。 */
export const TASK_CC_PREFIX = 'task-cc:';


/** ：gate-verdict relay 的事实：结论和它判的那个任务 id。 */
export function gateVerdictOf(
  message: unknown,
): { taskId: string; conclusion: 'pass' | 'needs_fix'; candidateReport: string } | null {
  if (typeof message !== 'object' || message === null) return null;
  const content = (message as { content?: unknown }).content;
  if (!Array.isArray(content)) return null;
  const text = content
    .map((block) =>
      typeof block === 'object' && block !== null && typeof (block as { text?: unknown }).text === 'string'
        ? (block as { text: string }).text
        : '',
    )
    .join('');
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof body !== 'object' || body === null) return null;
  const record = body as {
    kind?: unknown;
    conclusion?: unknown;
    candidate?: { task?: { id?: unknown }; report_id?: unknown };
  };
  if (record.kind !== 'gate_verdict') return null;
  const taskId = record.candidate?.task?.id;
  const conclusion = record.conclusion;
  const candidateReport = record.candidate?.report_id;
  if (typeof taskId !== 'string' || taskId === '') return null;
  if (conclusion !== 'pass' && conclusion !== 'needs_fix') return null;
  if (typeof candidateReport !== 'string' || candidateReport === '') return null;
  return { taskId, conclusion, candidateReport };
}


/** ：upstream_changed relay 的事实：哪个下游要按上游新结果核对。 */
export function upstreamChangedOf(message: unknown): { downstreamId: string } | null {
  if (typeof message !== 'object' || message === null) return null;
  const content = (message as { content?: unknown }).content;
  if (!Array.isArray(content)) return null;
  const text = content
    .map((block) =>
      typeof block === 'object' && block !== null && typeof (block as { text?: unknown }).text === 'string'
        ? (block as { text: string }).text
        : '',
    )
    .join('');
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof body !== 'object' || body === null) return null;
  const record = body as { kind?: unknown; downstream?: { id?: unknown } };
  if (record.kind !== 'upstream_changed') return null;
  const id = record.downstream?.id;
  if (typeof id !== 'string' || id === '') return null;
  return { downstreamId: id };
}


/** 消息的 source.rpcId（只认 source.kind=user）。 */
export function userRpcIdOf(message: unknown): string | null {
  if (!message || typeof message !== 'object') return null;
  const source = (message as { source?: unknown }).source;
  if (!source || typeof source !== 'object') return null;
  const typed = source as { kind?: unknown; rpcId?: unknown };
  if (typed.kind !== 'user' || typeof typed.rpcId !== 'string') return null;
  return typed.rpcId;
}


/** 消息里的文字块（抄送正文是一段 JSON 文本）。 */
export function messageTextOf(message: unknown): string {
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
}


/** ：一条 task-cc 用户消息 → 抄送事实；形状不对就当普通消息。 */
export function taskCcOfMessage(message: unknown): TaskCcView | null {
  const rpcId = userRpcIdOf(message);
  if (!rpcId || !rpcId.startsWith(TASK_CC_PREFIX)) return null;
  const raw = messageTextOf(message);
  if (!raw) return null;
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!body || typeof body !== 'object') return null;
  const typed = body as { kind?: unknown; task?: unknown; text?: unknown; chars?: unknown };
  if (typed.kind !== 'task_cc' || !typed.task || typeof typed.task !== 'object') return null;
  const task = typed.task as { id?: unknown; title?: unknown };
  if (typeof task.id !== 'string' || typeof task.title !== 'string') return null;
  if (typeof typed.text !== 'string') return null;
  return {
    inputId: rpcId.slice(TASK_CC_PREFIX.length),
    taskId: task.id,
    title: task.title,
    text: typed.text,
    ...(typeof typed.chars === 'number' ? { chars: typed.chars } : {}),
  };
}


/**
 * ：从日志里取抄送 Kind 的记录（按 inputId 去重，保留最早出现的时间）。
 *
 * Inject 不唤醒时消息先落 agent/inbox/spliced，被下一次请求领走后才多一条 user/message；
 * 两处都收，按 request id 去重，页面才在抄送发生时就画出来。
 */
export function taskCcFacts(events: ServiceEvent[]): { cc: TaskCcView; at: number }[] {
  const found = new Map<string, { cc: TaskCcView; at: number }>();
  for (const event of events) {
    let messages: unknown[] = [];
    if (event.type === 'user/message') {
      messages = [event.data?.message];
    } else if (event.type === 'agent/inbox/spliced') {
      const inserted = event.data?.inserted;
      messages = Array.isArray(inserted) ? inserted : [];
    } else {
      continue;
    }
    for (const message of messages) {
      const cc = taskCcOfMessage(message);
      if (!cc || found.has(cc.inputId)) continue;
      found.set(cc.inputId, { cc, at: event.time / 1000 });
    }
  }
  return [...found.values()];
}


/** ：一条抄送事实 → 对话里的单独一行（不是一轮，关联字段全空）。 */
export function ccViewOf(cc: TaskCcView, at: number): TurnViewData {
  return {
    id: 'task-cc-' + cc.inputId,
    startedAt: at,
    durationMs: 0,
    status: 'completed',
    user: null,
    cc,
    taskResults: [],
    says: [],
    reasoning: null,
    tools: [],
    answer: null,
    thinkingSeconds: 0,
    tokens: { input: 0, output: 0 },
    model: '',
    cost: null,
    retry: null,
    failure: null,
    question: null,
    compaction: null,
    canvas: null,
    card: null,
    delivery: null,
    stoppedAt: null,
  };
}
