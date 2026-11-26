// T090.1：输入队列投影。
//
// 停止时内核照 DSH clear 清掉两侧待领队列；删除落日志的 splice 只带
// target/start/removedCount/outcome，不带被删消息的 id 和正文。页面用日志里已有的
// inserted 折叠出投影，把被删的用户消息找回来放回输入框（文字、粘贴、附件、结果卡
// 按内容块还原）。不加协议：seq283 的 inserted 带整条消息，seq294 只带坐标和数量，
// 按顺序折叠能对上。

import type { ServiceEvent } from '@/data/service';

/** 日志里的一条消息（结构照 model::Message 的 JSON 形状）。 */
export interface InboxMessage {
  id?: string;
  role?: string;
  content?: InboxBlock[];
  source?: { kind?: string; rpcId?: string };
}

interface InboxBlock {
  type?: string;
  text?: string;
  paste?: boolean;
  canvas?: unknown;
  attachmentId?: string;
  name?: string;
  bytes?: number;
  taskId?: string;
  title?: string;
  version?: number;
}

/** 一条 canceled splice 要从日志找回来放回输入框的内容。 */
export interface QueueRestore {
  seq: number;
  text: string;
  pastes: string[];
  attachments: { id: string; name: string; bytes: number }[];
  resultCard?: { taskId: string; title: string; version: number };
}

/** 两个待领队列的投影（只留折叠需要的消息字段）。 */
export interface InboxProjection {
  nextStep: InboxMessage[];
  nextTurn: InboxMessage[];
}

export function emptyProjection(): InboxProjection {
  return { nextStep: [], nextTurn: [] };
}

/** 应用一条 splice；取消（outcome=canceled）时返回被删用户消息的放回内容。 */
export function applyInboxEvent(projection: InboxProjection, event: ServiceEvent): QueueRestore[] {
  if (event.type !== 'agent/inbox/spliced') return [];
  const data = event.data as {
    target?: unknown;
    start?: unknown;
    removedCount?: unknown;
    inserted?: unknown;
    outcome?: unknown;
  };
  const queue =
    data.target === 'next-step'
      ? projection.nextStep
      : data.target === 'next-turn'
        ? projection.nextTurn
        : null;
  if (!queue) return [];
  const start = Math.max(0, Math.min(Number(data.start ?? 0), queue.length));
  const removedCount = Math.max(0, Math.min(Number(data.removedCount ?? 0), queue.length - start));
  const removed = queue.splice(start, removedCount);
  const inserted = Array.isArray(data.inserted) ? (data.inserted as InboxMessage[]) : [];
  if (inserted.length > 0) queue.splice(start, 0, ...inserted);
  if (data.outcome !== 'canceled') return [];
  const restore = restoreOf(removed, event.seq);
  return restore ? [restore] : [];
}

/** 把一页日志折叠成投影，并收集打开页面前就已经发生的取消（打开时补放回）。 */
export function seedProjection(events: ServiceEvent[]): {
  projection: InboxProjection;
  restores: QueueRestore[];
} {
  const projection = emptyProjection();
  const restores: QueueRestore[] = [];
  for (const event of events) restores.push(...applyInboxEvent(projection, event));
  return { projection, restores };
}

/**
 * 只放回用户手打的发送：程序消息（date-now、任务抄送、任务报回等）的 rpcId 带冒号前缀，
 * 用户发送的 rpcId 是 uuid（不含冒号）。
 */
function isTypedByUser(message: InboxMessage): boolean {
  const rpcId = message.source?.rpcId;
  return (
    message.role === 'user' &&
    typeof rpcId === 'string' &&
    rpcId.length > 0 &&
    !rpcId.includes(':')
  );
}

/** 被删消息 → 放回输入框的内容；没有可放回的东西时返回 null。 */
function restoreOf(removed: InboxMessage[], seq: number): QueueRestore | null {
  const text: string[] = [];
  const pastes: string[] = [];
  const attachments: QueueRestore['attachments'] = [];
  let resultCard: QueueRestore['resultCard'];
  for (const message of removed) {
    if (!isTypedByUser(message)) continue;
    for (const block of message.content ?? []) {
      if (block.type === 'text' && typeof block.text === 'string') {
        if (block.paste) pastes.push(block.text);
        else text.push(block.text);
      } else if (
        block.type === 'file' &&
        typeof block.attachmentId === 'string' &&
        typeof block.name === 'string' &&
        typeof block.bytes === 'number'
      ) {
        attachments.push({ id: block.attachmentId, name: block.name, bytes: block.bytes });
      } else if (
        block.type === 'result-card' &&
        typeof block.taskId === 'string' &&
        typeof block.title === 'string' &&
        typeof block.version === 'number'
      ) {
        resultCard = { taskId: block.taskId, title: block.title, version: block.version };
      }
    }
  }
  if (text.length === 0 && pastes.length === 0 && attachments.length === 0 && !resultCard) return null;
  return {
    seq,
    text: text.join(String.fromCharCode(10)),
    pastes,
    attachments,
    ...(resultCard ? { resultCard } : {}),
  };
}
