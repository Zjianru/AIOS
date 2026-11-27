// 输入队列投影。
//
// 停止时内核照 DSH clear 清掉两侧待领队列；删除落日志的 splice 只带
// target/start/removedCount/outcome，不带被删消息的 id 和正文。页面用日志里已有的
// inserted 折叠出投影，把被删的用户消息找回来放回输入框（文字、粘贴、附件、结果卡
// 按内容块还原）。不加协议：seq283 的 inserted 带整条消息，seq294 只带坐标和数量，
// 按顺序折叠能对上。

import type { ServiceEvent } from '@/data/service';
import { mimeFromFileName } from './fileMime.ts';

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
  canvas?: { id?: unknown; title?: unknown };
  attachmentId?: string;
  name?: string;
  bytes?: number;
  mime?: string | null;
  taskId?: string;
  title?: string;
  version?: number;
}

/** 一条 canceled splice 要从日志找回来放回输入框的内容。 */
export interface QueueRestore {
  seq: number;
  /** 同一条 splice 里第几条消息（逐条放回的身份；seq+index 唯一）。 */
  key: string;
  text: string;
  pastes: string[];
  attachments: { id: string; name: string; bytes: number; mime?: string | null }[];
  /** （N2）：一次放回的多张结果卡/多个画布标记，按原顺序。 */
  resultCards: { taskId: string; title: string; version: number }[];
  canvases: { id: string; title: string }[];
}

/** 两个待领队列的投影（只留折叠需要的消息字段）。 */
export interface InboxProjection {
  nextStep: InboxMessage[];
  nextTurn: InboxMessage[];
}

export function emptyProjection(): InboxProjection {
  return { nextStep: [], nextTurn: [] };
}

/** 一条 splice 的折叠结果：取消要放回的内容 + 删除范围超出投影的条数。 */
export interface InboxFold {
  restores: QueueRestore[];
  /** >0 表示投影里还没有这几条（要往前翻页再折）。 */
  missing: number;
}

/** 应用一条 splice；取消（outcome=canceled）时返回被删用户消息的放回内容。 */
export function applyInboxEvent(projection: InboxProjection, event: ServiceEvent): InboxFold {
  if (event.type !== 'agent/inbox/spliced') return { restores: [], missing: 0 };
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
  if (!queue) return { restores: [], missing: 0 };
  const requestedStart = Math.max(0, Number(data.start ?? 0));
  const removedCount = Math.max(0, Number(data.removedCount ?? 0));
  // 删除范围超出投影里已有的消息：说明被删的那几条插在更早的日志页，先记缺多少，
  // 由调用方往前翻页重折（DSH 的 splice 坐标在写日志时都是合法范围）。
  const missing = Math.max(0, requestedStart + removedCount - queue.length);
  const start = Math.min(requestedStart, queue.length);
  const count = Math.min(removedCount, queue.length - start);
  const removed = queue.splice(start, count);
  const inserted = Array.isArray(data.inserted) ? (data.inserted as InboxMessage[]) : [];
  if (inserted.length > 0) queue.splice(start, 0, ...inserted);
  if (data.outcome !== 'canceled' || missing > 0) return { restores: [], missing };
  // N2：被清的消息逐条放回，不再合并成一条（每条的卡/画布各自成组）。
  const restores = removed
    .map((message, index) => restoreOf(message, event.seq, index))
    .filter((restore): restore is QueueRestore => restore !== null);
  return { restores, missing: 0 };
}

/** 一页日志（首屏或往前翻的一页）。 */
export interface ProjectionPage {
  events: ServiceEvent[];
  hasMore: boolean;
}

/**
 * 把若干页日志按「旧页在前」折叠成投影，并收集 requireFromSeq 之后的取消。
 * requireFromSeq 之前的页只用来补队列状态，不产生放回、也不算缺（更早的取消不重放）。
 */
export function foldProjectionPages(
  pages: ServiceEvent[][],
  requireFromSeq: number,
): { projection: InboxProjection; restores: QueueRestore[]; missing: number } {
  const projection = emptyProjection();
  const restores: QueueRestore[] = [];
  let missing = 0;
  for (const events of pages) {
    for (const event of events) {
      const folded = applyInboxEvent(projection, event);
      if (event.seq < requireFromSeq) continue;
      missing += folded.missing;
      restores.push(...folded.restores);
    }
  }
  return { projection, restores, missing };
}

/**
 * 首屏页 + 按需往前翻：canceled splice 的删除范围超出投影时，用同一个 beforeSeq 读口
 * 往前读一页、折进投影再对；对上或读到日志开头才停。往前读的页只折投影，不进对话列表。
 */
export async function loadProjection(
  first: ProjectionPage,
  fetchOlder: (beforeSeq: number) => Promise<ProjectionPage>,
): Promise<{ projection: InboxProjection; restores: QueueRestore[]; missing: number }> {
  const requireFromSeq = first.events[0]?.seq ?? 0;
  const pages: ServiceEvent[][] = [first.events];
  let oldest = first.events[0]?.seq ?? null;
  let hasMore = first.hasMore;
  let folded = foldProjectionPages(pages, requireFromSeq);
  while (folded.missing > 0 && hasMore && oldest !== null) {
    const older = await fetchOlder(oldest);
    if (older.events.length === 0) break;
    pages.unshift(older.events);
    oldest = older.events[0]?.seq ?? oldest;
    hasMore = older.hasMore;
    folded = foldProjectionPages(pages, requireFromSeq);
  }
  return folded;
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

/**
 * 被删消息 → 放回输入框的内容；没有可放回的东西时返回 null。
 *
 * 只还原用户发送时能拼出的块：text（含 paste）、file、result-card、带 canvas 的 text。
 * 其余类型（reasoning/tool-call 等）不会出现在用户收件箱里；真遇到就忽略，不猜、不丢正文。
 */
function restoreOf(message: InboxMessage, seq: number, index: number): QueueRestore | null {
  const text: string[] = [];
  const pastes: string[] = [];
  const attachments: QueueRestore['attachments'] = [];
  const resultCards: QueueRestore['resultCards'] = [];
  const canvases: QueueRestore['canvases'] = [];
  if (!isTypedByUser(message)) return null;
  for (const block of message.content ?? []) {
      if (block.type === 'text' && typeof block.text === 'string') {
        // 画布块：正文是发送时自动拼的「（来自画布「标题」）」；放回时恢复标记（N2 多条各留一个 chip），
        // 不把它当普通文字再拼一遍。
        if (
          block.canvas &&
          typeof block.canvas.id === 'string' &&
          typeof block.canvas.title === 'string'
        ) {
          canvases.push({ id: block.canvas.id, title: block.canvas.title });
        } else if (block.paste) pastes.push(block.text);
        else text.push(block.text);
      } else if (
        (block.type === 'file' || block.type === 'image') &&
        typeof block.attachmentId === 'string' &&
        typeof block.name === 'string' &&
        typeof block.bytes === 'number'
      ) {
        attachments.push({
          id: block.attachmentId,
          name: block.name,
          bytes: block.bytes,
          // 旧日志没有 mime：按扩展名兜底（png/jpg 等回图片类型），认不出才 null。
          mime:
            typeof block.mime === 'string' && block.mime.length > 0
              ? block.mime
              : mimeFromFileName(block.name),
        });
      } else if (
        block.type === 'result-card' &&
        typeof block.taskId === 'string' &&
        typeof block.title === 'string' &&
        typeof block.version === 'number'
      ) {
        resultCards.push({ taskId: block.taskId, title: block.title, version: block.version });
      }
  }
  if (
    text.length === 0 &&
    pastes.length === 0 &&
    attachments.length === 0 &&
    resultCards.length === 0 &&
    canvases.length === 0
  ) {
    return null;
  }
  return {
    seq,
    key: seq + '-' + index,
    text: text.join(String.fromCharCode(10)),
    pastes,
    attachments,
    resultCards,
    canvases,
  };
}
