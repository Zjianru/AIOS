//! 抄送 Kind 的行按时间插进列表。

import type { ServiceEvent } from '@/data/service';
import { ccViewOf, taskCcFacts } from './turnViews';
import type { TurnViewData } from './turnViews';

/**
 * ：把抄送 Kind 的行按时间插进列表。
 *
 * 抄送不是一轮：单独一条 view（cc 字段），按发生时间插在第一个更晚的轮之前；
 * 同一条 request id 只成一行（inbox/spliced 与领取后的 user/message 都收）。
 */
export function mergeCcViews(views: TurnViewData[], events: ServiceEvent[]): TurnViewData[] {
  const facts = taskCcFacts(events);
  if (facts.length === 0) return views;
  let next = views;
  for (const { cc, at } of facts) {
    const id = 'task-cc-' + cc.inputId;
    if (next.some((view) => view.id === id)) continue;
    const view = ccViewOf(cc, at);
    const index = next.findIndex((item) => item.startedAt !== null && item.startedAt > at);
    next = index < 0 ? next.concat(view) : next.slice(0, index).concat(view, next.slice(index));
  }
  return next;
}
