/**
 * 把服务端 calendar-read 的事实翻成日历页的词和颜色。
 *
 * 只有服务端有的事实：约定、版本、下次触发、运行记录、服务端算好的周计划点。
 * 页面不自己算规则、不补运行记录；没变（skipped）只在约定视图里出现，周视图不画。
 */

import type { CalendarAppointmentView } from '@/data/generated/CalendarAppointmentView';
import type { CalendarReadResponse } from '@/data/generated/CalendarReadResponse';
import type { CalendarRunView } from '@/data/generated/CalendarRunView';
import type { CalendarWeekPoint } from '@/data/generated/CalendarWeekPoint';

export type ApptKind = 'time' | 'watch' | 'loop' | 'builtin';
export type ApptState = 'active' | 'paused' | 'waiting' | 'ended' | 'cancelled';
export type RunResult = 'queued' | 'sent' | 'skipped' | 'failed';

export interface ApptRun {
  id: number;
  at: number;
  result: RunResult;
  /** 送 Kind 的那一轮；点运行点回到那一轮。 */
  turn: number | null;
  note: string;
  merged: number;
  manual: boolean;
  trigger: string;
}

export interface Appointment {
  id: string;
  kind: ApptKind;
  words: string;
  rule: string;
  timezone: string;
  status: string;
  state: ApptState;
  stateWhy: string | null;
  binding: string;
  boundTaskId: string | null;
  version: number;
  next: number | null;
  createdAt: number;
  watchKind: string | null;
  watchTarget: string | null;
  cycleTaskId: string | null;
  cycleMaxRounds: number | null;
  runs: ApptRun[];
}

export const GROUP_ORDER: { key: ApptKind; label: string }[] = [
  { key: 'time', label: '到点' },
  { key: 'watch', label: '盯着变化' },
  { key: 'loop', label: '循环任务' },
  { key: 'builtin', label: '内置' },
];

export const STATE_WORD: Record<ApptState, string> = {
  active: '在用',
  paused: '已暂停',
  waiting: '等你',
  ended: '已结束',
  cancelled: '已取消',
};

export const RESULT_WORD: Record<RunResult, string> = {
  queued: '排着',
  sent: '送了',
  skipped: '跳过',
  failed: '没送成',
};

export const RESULT_TONE: Record<RunResult, string> = {
  queued: 'var(--wait)',
  sent: 'var(--done)',
  skipped: 'var(--ink-3)',
  failed: 'var(--fail)',
};

export function kindOf(appointment: CalendarAppointmentView): ApptKind {
  if (appointment.id === 'builtin:dream') return 'builtin';
  if (appointment.cycleTaskId) return 'loop';
  if (appointment.watchKind) return 'watch';
  return 'time';
}

export function stateOf(appointment: CalendarAppointmentView): ApptState {
  if (appointment.id === 'builtin:dream') {
    if (appointment.status === 'paused') return 'paused';
    return 'active';
  }
  if (appointment.status === 'paused') {
    return appointment.pauseReason ? 'waiting' : 'paused';
  }
  if (appointment.status === 'cancelled') return 'cancelled';
  if (appointment.status === 'ended') return 'ended';
  return 'active';
}

export function runOf(run: CalendarRunView): ApptRun {
  return {
    id: run.id,
    at: run.nominalAt,
    result: run.result as RunResult,
    turn: run.sentTurn ?? null,
    note: run.reason,
    merged: run.mergedCount,
    manual: run.manual,
    trigger: run.trigger,
  };
}

export function appointmentOf(view: CalendarAppointmentView): Appointment {
  return {
    id: view.id,
    kind: kindOf(view),
    words: view.condition,
    rule: view.rule,
    timezone: view.timezone,
    status: view.status,
    state: stateOf(view),
    stateWhy: view.pauseReason ?? null,
    binding: view.binding,
    boundTaskId: view.boundTaskId ?? null,
    version: view.version,
    next: view.nextTriggerAt ?? null,
    createdAt: view.createdAt,
    watchKind: view.watchKind ?? null,
    watchTarget: view.watchTarget ?? null,
    cycleTaskId: view.cycleTaskId ?? null,
    cycleMaxRounds: view.cycleMaxRounds ?? null,
    runs: view.runs.map(runOf),
  };
}

export function appointmentsOf(response: CalendarReadResponse | null): Appointment[] {
  return (response?.appointments ?? []).map(appointmentOf);
}

/** 运行点：约定视图按 id 升序（服务端给的是时间序）；跳过也画，周视图自己筛。 */
export function recentRuns(appointment: Appointment, limit = 8): ApptRun[] {
  return appointment.runs.slice(-limit);
}

export function stampOf(ms: number): string {
  if (!ms) return '';
  const date = new Date(ms);
  return String(date.getMonth() + 1) + '/' + date.getDate() + ' ' + String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

export function clockOf(ms: number): string {
  if (!ms) return '';
  const date = new Date(ms);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

/** 本地周一 00:00 的毫秒时间戳；周视图的 week 参数。 */
export function weekStartOf(offsetWeeks = 0): number {
  const now = new Date();
  const day = (now.getDay() + 6) % 7;
  const monday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - day + offsetWeeks * 7);
  return monday.getTime();
}

/** 一周七列（周一起）；每列是本地日 00:00。 */
export function daysOfWeek(weekStart: number): number[] {
  return Array.from({ length: 7 }, (_, index) => {
    const date = new Date(weekStart);
    date.setDate(date.getDate() + index);
    return date.getTime();
  });
}

export interface WeekCell {
  plan: CalendarWeekPoint[];
  run: CalendarWeekPoint[];
}

/** 按列分周计划点/运行点；周视图不画 skipped（没变/跳过），只画计划与送过/失败/排着的。 */
export function weekCells(response: CalendarReadResponse | null, weekStart: number): WeekCell[] {
  const days = daysOfWeek(weekStart);
  const cells: WeekCell[] = days.map(() => ({ plan: [], run: [] }));
  for (const point of response?.week ?? []) {
    const index = days.findIndex((day) => point.at >= day && point.at < day + 86_400_000);
    if (index < 0) continue;
    if (point.kind === 'plan') cells[index].plan.push(point);
    else if (point.kind === 'run') {
      if (point.result === 'skipped' || point.result === 'queued') continue;
      cells[index].run.push(point);
    }
  }
  return cells;
}
