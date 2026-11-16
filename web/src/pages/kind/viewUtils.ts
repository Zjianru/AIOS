//! 视图工具：分页常量、日期分组、索引与画布标题。

import { dayLabelOf } from '@/lib/numbers';
import type { TurnViewData } from './turnViews';

export const PAGE_LAST = 5;


export const PAGE_OLDER = 20;


export const FIRST_INDEX_BASE = 1_000_000;


export function dayKeyOf(seconds: number | null): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  return date.getFullYear() + '-' + date.getMonth() + '-' + date.getDate();
}


/** 一次性算好日期分隔与「上次查看之后」的位置，列表渲染时只查索引，不在数组里找。 */
export function decorate(views: TurnViewData[]): {
  dayLabels: Map<string, string>;
  lastSeenId: string | null;
  lastSeenCounts: { moved: number; waiting: number };
} {
  const dayLabels = new Map<string, string>();
  let previous = '';
  for (const view of views) {
    const key = dayKeyOf(view.startedAt);
    if (key !== previous) {
      dayLabels.set(view.id, dayLabelOf(view.startedAt));
      previous = key;
    }
  }
  const lastSeenId = views.length > 4 ? views[views.length - 4].id : null;
  return { dayLabels, lastSeenId, lastSeenCounts: { moved: 3, waiting: 5 } };
}


export function indexOf(views: TurnViewData[]): Map<string, TurnViewData> {
  const map = new Map<string, TurnViewData>();
  for (const view of views) map.set(view.id, view);
  return map;
}


/** 从还没流完的参数 JSON 里取 canvas 的 title；还没流到（或半个转义）就先给空串。 */
export function canvasTitleOf(partial: string): string {
  const match = /"title"\s*:\s*"((?:[^"\\]|\\.)*)"/.exec(partial);
  if (!match) return '';
  try {
    return JSON.parse('"' + match[1] + '"') as string;
  } catch {
    return '';
  }
}
