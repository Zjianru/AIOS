import { stageLabel } from '../work/workModel';

/** 派出/自办块的状态词只从看板这一张阶段词表出。 */
export function blockWord(block: { stage?: string | null; status: string }): string {
  return block.stage ? stageLabel(block.stage) : block.status;
}

/**
 * 把服务端 calendar-read 的事实翻成日历页的词和颜色。
 *
 * 只有服务端有的事实：约定、版本、下次触发、运行记录、服务端算好的周计划点。
 * 页面不自己算规则、不补运行记录；没变（skipped）只在约定视图里出现，周视图不画。
 */

import type { CalendarAppointmentView } from '@/data/generated/CalendarAppointmentView';
import type { CalendarReadResponse } from '@/data/generated/CalendarReadResponse';
import type { CalendarRunView } from '@/data/generated/CalendarRunView';
import type { CalendarRunBlock } from '@/data/generated/CalendarRunBlock';
import type { CalendarPlanBlock } from '@/data/generated/CalendarPlanBlock';
import type { CalendarDenseDay } from '@/data/generated/CalendarDenseDay';
import type { CalendarRuleColor } from '@/data/generated/CalendarRuleColor';

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
  trigger: string;
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
  sourceTurn: number | null;
  durationMinutes: number | null;
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
    trigger: view.trigger,
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
    sourceTurn: view.sourceTurn ?? null,
    durationMinutes: view.durationMinutes ?? null,
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

/** 约定自己的时区里显示「下次触发」（确认卡/详情不跟浏览器时区漂）。 */
export function stampInZone(ms: number, timezone: string): string {
  if (!ms) return '';
  try {
    const parts = new Intl.DateTimeFormat('zh-CN', {
      timeZone: timezone,
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    }).formatToParts(new Date(ms));
    const value = (type: string) => parts.find((part) => part.type === type)?.value ?? '';
    return value('month') + '/' + value('day') + ' ' + value('hour') + ':' + value('minute');
  } catch {
    return stampOf(ms);
  }
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

export interface RangeBlock {
  id: number;
  appointmentId: string;
  title: string;
  start: number;
  end: number;
  stage: string | null;
  status: string;
  taskId: string | null;
  turn: number | null;
}

export interface PlanBlock {
  appointmentId: string;
  title: string;
  start: number;
  end: number;
  color: number;
}

export interface DenseDay {
  appointmentId: string;
  title: string;
  day: number;
  runs: number;
  unchanged: number;
}

export function rangeBlocksOf(response: CalendarReadResponse | null): RangeBlock[] {
  return (response?.ranges ?? []).map((block: CalendarRunBlock) => ({
    id: block.id,
    appointmentId: block.appointmentId,
    title: block.title,
    start: block.start,
    end: block.end,
    stage: block.stage ?? null,
    status: block.status,
    taskId: block.taskId ?? null,
    turn: block.turn ?? null,
  }));
}

export function plansOf(response: CalendarReadResponse | null): PlanBlock[] {
  return (response?.plans ?? []).map((plan: CalendarPlanBlock) => ({
    appointmentId: plan.appointmentId,
    title: plan.title,
    start: plan.start,
    end: plan.end,
    color: plan.color,
  }));
}

export function denseOf(response: CalendarReadResponse | null): DenseDay[] {
  return (response?.dense ?? []).map((day: CalendarDenseDay) => ({
    appointmentId: day.appointmentId,
    title: day.title,
    day: day.day,
    runs: day.runs,
    unchanged: day.unchanged,
  }));
}

export function colorsOf(response: CalendarReadResponse | null): Map<string, number> {
  const colors = new Map<string, number>();
  for (const color of (response?.colors ?? []) as CalendarRuleColor[]) {
    colors.set(color.appointmentId, color.color);
  }
  return colors;
}

/** 某个本地日起点（含 dayStart）；dayStart 是该日 00:00。 */
export function dayStartOf(ms: number): number {
  const date = new Date(ms);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** 本地月份 1 号 00:00。 */
export function monthStartOf(offsetMonths = 0): number {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth() + offsetMonths, 1).getTime();
}

/** 月视图 6x7 格；从 1 号所在周（周一起）前一天开始到第 42 格。 */
export function monthCells(monthStart: number): number[] {
  const first = new Date(monthStart);
  const weekday = (first.getDay() + 6) % 7;
  const gridStart = new Date(first.getFullYear(), first.getMonth(), 1 - weekday).getTime();
  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(gridStart);
    date.setDate(date.getDate() + index);
    return date.getTime();
  });
}

export function ruleColor(color: number | undefined): string {
  const palette = ['#5b8def', '#e0a43b', '#4ea36d', '#d96d6d', '#8b6fd9', '#5fa8b8'];
  return palette[(color ?? 0) % palette.length];
}
