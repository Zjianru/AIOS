/**
 * 任务对话的 live 合并与收尾派生（纯函数，页面和 node 用例共用）。
 *
 * 日志事件按 seq 单调；重连、补齐和 live 可能把同一段事件送两次，页面只认一次。
 * 终态报告后执行者会立刻收掉会话，日志可能没有 turn/end：头部带来的 reportedAt
 * 只能收尾它之前开始的那一轮，不能把 waiting 报告之后 task_status 起的新一轮标成完成。
 */

import type { ServiceEvent } from '@/data/service';
import type { TurnViewData } from '@/pages/kind/turnViews';

/** 页面已显示的事件按 seq 有序；重复或倒退的事件丢掉（补齐 + live 的边界）。 */
export function mergeLiveEvent(events: ServiceEvent[], event: ServiceEvent): ServiceEvent[] {
  const last = events.length > 0 ? events[events.length - 1].seq : null;
  if (last !== null && event.seq <= last) return events;
  return [...events, event];
}

/** 用头部的最近报告时间收尾最后一轮；报告必须不早于这一轮开始。 */
export function withReportedCompletion(
  views: TurnViewData[],
  reportedAt: number | null | undefined,
): TurnViewData[] {
  if (reportedAt === null || reportedAt === undefined || views.length === 0) return views;
  const last = views.length - 1;
  if (views[last].status !== 'inProgress') return views;
  const startedAt = views[last].startedAt;
  if (startedAt === null) return views;
  const durationMs = Math.max(0, reportedAt - startedAt * 1000);
  return views.map((view, index) =>
    index === last ? { ...view, status: 'completed' as const, durationMs } : view,
  );
}
