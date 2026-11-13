/**
 * T041：Kind 任务卡对「未终态任务」的状态词从 task-read 映射（和任务对话页头同一事实源）。
 *
 * Kind 卡原本只从 relay 派生；「停下这一轮」（task-interrupt）只中断当前轮、不发 relay，
 * 于是任务库已经不 running、任务对话页头显示「等待中」，Kind 卡却停在「进行中」。
 * 这里只补「已派出、还没被 relay 更新」的卡：running→进行中、stopped→已停止、
 * interrupted→被打断、reportedAt 有值等 relay 的结果状态、其余→等待中。
 */
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import type { TaskResultFact } from '../../data/taskResult.ts';
import type { TurnViewData } from './turnViews.ts';

/** 派发卡里还需要任务库事实的那几种状态（relay 已给报告状态的卡不补）。 */
const NON_TERMINAL_STATES: ReadonlySet<string> = new Set(['dispatching', 'dispatched', 'pending']);

/** 这张卡要不要用 task-read 事实补状态：已派出、有 task id、还没被 relay 更新。 */
export function needsTaskState(fact: TaskResultFact): boolean {
  return Boolean(fact.taskId) && fact.fromDispatch === true && NON_TERMINAL_STATES.has(fact.state);
}

/** 任务库事实 → 派发卡状态键（与任务对话页头的状态同一个判断顺序）。 */
export function taskStateOf(
  task: TaskReadHeader,
): 'dispatched' | 'stopped' | 'interrupted' | 'pending' {
  if (task.running) return 'dispatched';
  if (task.stopped) return 'stopped';
  if (task.interrupted) return 'interrupted';
  return 'pending';
}

/** 页面上还有哪些任务需要拉 task-read（非终态卡的 task id，去重）。 */
export function activeTaskIds(views: TurnViewData[]): string[] {
  const ids = new Set<string>();
  for (const view of views) {
    for (const fact of view.taskResults) {
      if (needsTaskState(fact)) ids.add(fact.taskId);
    }
  }
  return [...ids];
}

/** 把 task-read 事实映射进非终态派发卡；没有变化就返回原数组（store 不白重渲染）。 */
export function applyTaskCardStates(
  views: TurnViewData[],
  tasks: ReadonlyMap<string, TaskReadHeader>,
): TurnViewData[] {
  if (tasks.size === 0) return views;
  let changedAny = false;
  const next = views.map((view) => {
    let changed = false;
    const taskResults = view.taskResults.map((fact) => {
      if (!needsTaskState(fact)) return fact;
      const task = tasks.get(fact.taskId);
      if (!task) return fact;
      // 汇报了：结果状态由 relay 给（有结果/待你核/已接受），不在这里猜。
      if (task.reportedAt != null) return fact;
      const state = taskStateOf(task);
      if (state === fact.state) return fact;
      changed = true;
      return { ...fact, state };
    });
    if (!changed) return view;
    changedAny = true;
    return { ...view, taskResults };
  });
  return changedAny ? next : views;
}
