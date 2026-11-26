// T095.5：双击 Esc 回退的前端数据（原文还原 + 菜单行）。
// 候选列表以服务 rewind list 为准（内核给 seq/time/一行），这里只做「seq → 本地轮」的映射
// 和「从日志消息还原原文（字/粘贴/附件/结果卡/画布标记）」。

import type { SendAttachmentRef } from '@/data/service';
import { mimeFromFileName } from './fileMime.ts';
import type { TurnViewData } from './viewTypes';

export interface RewindSource {
  text: string;
  pastes: string[];
  files: SendAttachmentRef[];
  resultCards: { taskId: string; title: string; version: number }[];
  canvases: { id: string; title: string }[];
}

/** 日志里一条 user/message 的块 → 放回输入框的内容（顺序照日志）。 */
export function rewindSourceOfMessage(message: unknown): RewindSource | null {
  if (!message || typeof message !== 'object') return null;
  const content = (message as { content?: unknown }).content;
  if (!Array.isArray(content)) return null;
  const source: RewindSource = { text: '', pastes: [], files: [], resultCards: [], canvases: [] };
  const text: string[] = [];
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
      mime?: unknown;
      taskId?: unknown;
      title?: unknown;
      version?: unknown;
    };
    if (typed.type === 'text') {
      const value = typeof typed.text === 'string' ? typed.text : '';
      const canvas = typed.canvas as { id?: unknown; title?: unknown } | undefined;
      if (canvas && typeof canvas.id === 'string' && typeof canvas.title === 'string') {
        source.canvases.push({ id: canvas.id, title: canvas.title });
      } else if (typed.paste === true) source.pastes.push(value);
      else text.push(value);
      continue;
    }
    if (typed.type === 'file' && typeof typed.attachmentId === 'string') {
      const name = typeof typed.name === 'string' ? typed.name : '';
      source.files.push({
        id: typed.attachmentId,
        name,
        bytes: typeof typed.bytes === 'number' ? typed.bytes : 0,
        mime: typeof typed.mime === 'string' && typed.mime ? typed.mime : mimeFromFileName(name),
      });
      continue;
    }
    if (
      typed.type === 'result-card' &&
      typeof typed.taskId === 'string' &&
      typeof typed.title === 'string'
    ) {
      source.resultCards.push({
        taskId: typed.taskId,
        title: typed.title,
        version: typeof typed.version === 'number' ? typed.version : 0,
      });
    }
  }
  source.text = text.join('');
  if (
    !source.text &&
    source.pastes.length === 0 &&
    source.files.length === 0 &&
    source.resultCards.length === 0 &&
    source.canvases.length === 0
  ) {
    return null;
  }
  return source;
}

/** 服务 rewind list 的一行（seq 是日志里那条 user/message 的 seq）。 */
export interface RewindRow {
  seq: number;
  time: number;
  text: string;
}

/** seq → 本地轮（恢复后要拿原文时用）。 */
export function viewOfSeq(views: TurnViewData[], seq: number): TurnViewData | undefined {
  return views.find((view) => view.user?.seq === seq);
}

/** 这一句前面还有没有可总结的（服务端会再校验；这里只管菜单灰不灰）。 */
export function hasBefore(rows: RewindRow[], seq: number | null): boolean {
  if (seq === null) return false;
  const index = rows.findIndex((row) => row.seq === seq);
  return index > 0;
}
