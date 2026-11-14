/**
 * T055：工作页/「现在」共用的纯派生（看板四列、卡上的说明和下一步、完成列筛选、本周完成）。
 *
 * 只读 Work 快照：不请求、不写库，规则集中在这里，两个页面算同一套。
 */

import type { TaskFact } from '@/data/generated/TaskFact';
import type { WaitItem } from '@/data/generated/WaitItem';

export type BoardColumn = 'queue' | 'doing' | 'wait' | 'done';

export const STAGE_LABEL: Record<string, string> = {
  needs_conditions: '待补足条件',
  pending: '待执行',
  running: '执行中',
  ready_for_review: '待检查',
  requirement_met: '任务要求已满足',
  closed: '已关闭',
};

export function stageLabel(stage: string): string {
  return STAGE_LABEL[stage] ?? stage;
}

export const WAIT_KIND_ORDER = ['resume', 'confirm', 'answer', 'judge', 'accept'] as const;
export type WaitKind = (typeof WAIT_KIND_ORDER)[number];

export const WAIT_LABEL: Record<string, string> = {
  resume: '接续',
  confirm: '要你确认',
  answer: '有问题要你答',
  judge: '里程碑等你判',
  accept: '结果等你接受',
};

/** 「等你」里每种能点的动作；accept 的按钮在组接受票里加。 */
export const WAIT_ACTION: Record<string, string | null> = {
  resume: '去看',
  confirm: '去看',
  answer: '去答',
  judge: '去判',
  accept: null,
};

export interface BoardTask {
  task: TaskFact;
  column: BoardColumn;
  /** 卡上状态词。 */
  word: string;
  /** 下一步谁来。 */
  next: string;
}

export interface BoardModel {
  queue: BoardTask[];
  doing: BoardTask[];
  /** 「等你」列直接用 WaitItem（和右栏同一份）。 */
  wait: WaitItem[];
  done: BoardTask[];
  /** 有「等你」项的任务 id。 */
  waitTaskIds: Set<string>;
}

export function formatAt(ms: number | null | undefined): string {
  if (!ms) return '';
  const date = new Date(ms);
  return (
    String(date.getMonth() + 1).padStart(2, '0') +
    '-' +
    String(date.getDate()).padStart(2, '0') +
    ' ' +
    String(date.getHours()).padStart(2, '0') +
    ':' +
    String(date.getMinutes()).padStart(2, '0')
  );
}

export function columnOf(task: TaskFact, waitTaskIds: Set<string>): BoardColumn {
  // 「等你」优先：有要你做的事先归等你（已关闭的除外）。
  if (task.stage === 'closed') return 'done';
  if (waitTaskIds.has(task.id)) return 'wait';
  if (task.stage === 'needs_conditions' || task.stage === 'pending') return 'queue';
  if (task.stage === 'running' || task.stage === 'ready_for_review') return 'doing';
  // requirement_met：门判完了，等结果被接受（属于等你那条链）。
  return 'wait';
}

function wordOf(task: TaskFact, column: BoardColumn, waitKind: string | undefined): string {
  switch (column) {
    case 'queue':
      if (task.stage === 'needs_conditions') return task.afterWaiting ? '等前置' : '缺条件';
      return task.after.length > 0 ? '等前置' : '等待中';
    case 'doing':
      if (task.stage === 'running') return '执行中';
      if (task.gateOf) return '等里程碑';
      return '检查中';
    case 'wait':
      return waitKind ? (WAIT_LABEL[waitKind] ?? '等你') : '等你';
    case 'done':
      return task.closedAt ? '关闭于 ' + formatAt(task.closedAt) : '已关闭';
  }
}

function nextOf(task: TaskFact, column: BoardColumn): string {
  switch (column) {
    case 'queue':
      return task.stage === 'needs_conditions'
        ? '等 Kind 补条件'
        : task.after.length > 0
          ? '等前置完成'
          : '等排程';
    case 'doing':
      return 'ThinAgent';
    case 'wait':
      return '你';
    case 'done':
      return '已完成';
  }
}

/** 从任务事实 +「等你」派生看板四列；同一张卡只在一列。 */
export function boardOf(tasks: TaskFact[], wait: WaitItem[]): BoardModel {
  const waitTaskIds = new Set<string>();
  for (const item of wait) {
    if (item.taskId) waitTaskIds.add(item.taskId);
  }
  const waitKindOf = new Map<string, string>();
  for (const item of wait) {
    if (item.taskId && !waitKindOf.has(item.taskId)) waitKindOf.set(item.taskId, item.kind);
  }
  const queue: BoardTask[] = [];
  const doing: BoardTask[] = [];
  const done: BoardTask[] = [];
  for (const task of tasks) {
    const column = columnOf(task, waitTaskIds);
    const waitKind = task.id ? waitKindOf.get(task.id) : undefined;
    const card: BoardTask = {
      task,
      column,
      word: wordOf(task, column, waitKind),
      next: nextOf(task, column),
    };
    if (column === 'queue') queue.push(card);
    else if (column === 'doing') doing.push(card);
    else if (column === 'done') done.push(card);
  }
  done.sort((left, right) => (right.task.closedAt ?? 0) - (left.task.closedAt ?? 0));
  return { queue, doing, wait, done, waitTaskIds };
}

export interface WaitGroup {
  kind: string;
  label: string;
  items: WaitItem[];
}

/** 按五种固定顺序分组，每组只出有的；组内保持服务端顺序（新的在上）。 */
export function waitGroups(wait: WaitItem[]): WaitGroup[] {
  return WAIT_KIND_ORDER.map((kind) => ({
    kind,
    label: WAIT_LABEL[kind],
    items: wait.filter((item) => item.kind === kind),
  })).filter((group) => group.items.length > 0);
}

export type DoneRange = 'today' | '7d' | '30d' | 'all';

export const DONE_RANGES: { key: DoneRange; label: string }[] = [
  { key: 'today', label: '今天' },
  { key: '7d', label: '近 7 天' },
  { key: '30d', label: '近 30 天' },
  { key: 'all', label: '全部' },
];

export const DONE_RANGE_KEY = 'aios.work.doneRange';

/** 本地时间的范围起点（今天 0 点 / 近 7 天 / 近 30 天）；全部 = null。 */
export function doneRangeStart(range: DoneRange, now: number): number | null {
  const date = new Date(now);
  switch (range) {
    case 'today':
      return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
    case '7d':
      return now - 7 * 24 * 60 * 60 * 1000;
    case '30d':
      return now - 30 * 24 * 60 * 60 * 1000;
    case 'all':
      return null;
  }
}

/** 完成列按显示范围筛；column 里留下的是「更早」的条数。 */
export function filterDone(
  done: BoardTask[],
  range: DoneRange,
  now: number,
): { visible: BoardTask[]; older: number } {
  const start = doneRangeStart(range, now);
  if (start === null) return { visible: done, older: 0 };
  const visible = done.filter((card) => (card.task.closedAt ?? 0) >= start);
  return { visible, older: done.length - visible.length };
}

/**
 * 「现在」：出了问题 = 待补足条件、不在「等你」里、也不是「等前置」的任务（行上固定「处理中」）。
 * 「等前置」是排队正常状态（D96 的「失败或缺条件、Kind 正在补」不算它）。
 */
export function problemsOf(tasks: TaskFact[], wait: WaitItem[]): TaskFact[] {
  const waiting = new Set(wait.flatMap((item) => (item.taskId ? [item.taskId] : [])));
  return tasks.filter(
    (task) => task.stage === 'needs_conditions' && !task.afterWaiting && !waiting.has(task.id),
  );
}

/** 「现在」：在推进 = 看板「在做」列的任务。 */
export function advancingOf(board: BoardModel): TaskFact[] {
  return board.doing.map((card) => card.task);
}

/** 本周完成：已关闭、非门、非总任务，closed_at 不早于本地本周一 0 点。 */
export function weekDone(tasks: TaskFact[], now: number): { count: number; names: string[] } {
  const date = new Date(now);
  const weekday = (date.getDay() + 6) % 7; // 周一 = 0
  const monday = new Date(date.getFullYear(), date.getMonth(), date.getDate() - weekday).getTime();
  const done = tasks
    .filter((task) => task.stage === 'closed' && !task.gate && (task.closedAt ?? 0) >= monday)
    .sort((left, right) => (right.closedAt ?? 0) - (left.closedAt ?? 0));
  return { count: done.length, names: done.slice(0, 4).map((task) => task.title) };
}
