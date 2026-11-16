//! 任务对话页的共享小工具：文本/时间/事件可见性/固定文案。

import { ServiceHttpError } from '@/data/service';
import type { ServiceEvent } from '@/data/service';

export const NO_CANVAS = () => {};


export const NO_COUNTS = { moved: 0, waiting: 0 };


/** 任务会话里的第一条用户消息是执行者写的 assignment；它不算用户气泡。 */
export const ASSIGNMENT_PREFIX = 'assignment:';


export const THIN_TOOLS = new Set([
  'read',
  'glob',
  'grep',
  'skill',
  'todo_write',
  'web_search',
  'web_fetch',
  'task_report',
  'task_handback',
  'task_ledger_note',
  'task_ledger_history',
]);


/** 任务对话里说的一句话的 request id 前缀（服务端 task-say:<inputId>）。 */
export const TASK_SAY_PREFIX = 'task-say:';


export function rpcIdOf(event: ServiceEvent): string | null {
  const message = event.data?.message;
  if (!message || typeof message !== 'object') return null;
  const source = (message as { source?: unknown }).source;
  if (!source || typeof source !== 'object') return null;
  const typed = source as { kind?: unknown; rpcId?: unknown };
  if (typed.kind !== 'user' || typeof typed.rpcId !== 'string') return null;
  return typed.rpcId;
}


/** 页面上不显示的 assignment 事件；其余事件照日志。 */
export function visibleTaskEvents(events: ServiceEvent[]): ServiceEvent[] {
  return events.filter(
    (event) => event.type !== 'user/message' || !rpcIdOf(event)?.startsWith(ASSIGNMENT_PREFIX),
  );
}


/** 服务错误 → 给人看的原因；task/unavailable 的正文里已经是可读事实。 */
export function reasonOf(error: unknown): string {
  if (error instanceof ServiceHttpError && error.detail) return error.detail;
  if (error instanceof Error && error.message) return error.message;
  return '未知原因';
}


export function toolsText(tools: string[]): string {
  if (tools.length === THIN_TOOLS.size && tools.every((tool) => THIN_TOOLS.has(tool))) {
    return '只读：读、搜文件、能力、计划、搜网页、读网页、记账本、查账本历史、上报、交回';
  }
  return tools.join('、');
}


export function clock(at: number): string {
  const date = new Date(at);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}
