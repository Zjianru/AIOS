/**
 * T038/T039：派发卡与 relay 卡的折卡（纯函数，node 用例可直接跑）。
 *
 * 派发卡（task_accept 派生）在派出那一轮；任务汇报/停止/被打断的 relay 可能在同一轮，
 * 也可能在别的轮或别的页（live 逐轮重推、历史按页派生）。这里按 task id 把派发后
 * 第一条 relay 并回派发卡：保留派发卡的位置，内容换成 relay 事实（id 用 relay 的，
 * 接受决定按 relay id 回填）；之后的 relay 各留各的卡（第 2 次 Attempt、新版本报告）。
 */
import type { TaskResultFact } from '../../data/taskResult.ts';
import type { TurnViewData } from './turnViews.ts';

/** 能不能把 fact 并到 existing 这张派发卡上；能就返回合并后的事实，不能返回 null。 */
export function foldedTaskCard(
  existing: TaskResultFact,
  fact: TaskResultFact,
): TaskResultFact | null {
  // 只有还没被报告更新过的派发卡能并（派出中/进行中/等待中都算）；之后的报告各出一张卡。
  if (
    existing.state !== 'dispatching' &&
    existing.state !== 'dispatched' &&
    existing.state !== 'pending'
  ) {
    return null;
  }
  return {
    ...fact,
    fromDispatch: true,
    // 写入范围是派出时冻结的事实：relay 没给就沿用它。
    ...(fact.writeScope === undefined && existing.writeScope !== undefined
      ? { writeScope: existing.writeScope }
      : {}),
  };
}

/**
 * 把同一任务的多张结果卡折成一张（跨轮、跨页用）。
 *
 * 只折派发后的第一条 relay：保留最早那张派发卡的位置，换成 relay 事实；后面那条
 * 不再折（各留各的卡）。纯 relay 的任务（旧会话、重启汇报）没有派发卡，原样不动。
 * 丢卡按数组位置记，不按事实 id —— replacement 的 id 就是 relay 的 id，按 id 过滤
 * 会把刚换上去的新卡也一起丢掉。
 *
 * 逐轮重推（refreshTurn）会把同一张 relay 再派生一遍：主卡已经换成这张 relay 时，
 * 后出现的那份是重复，按位置丢掉，不能当「新报告」留成第二张卡。
 */
export function reconcileTaskCards(views: TurnViewData[]): TurnViewData[] {
  const positionKey = (viewIndex: number, factIndex: number): string =>
    viewIndex + ':' + String(factIndex);
  const homes = new Map<string, { viewIndex: number; factIndex: number }>();
  const folded = new Set<string>();
  const dropped = new Set<string>();
  const replacements = new Map<string, TaskResultFact>();
  views.forEach((view, viewIndex) => {
    view.taskResults.forEach((fact, factIndex) => {
      if (!fact.taskId || folded.has(fact.taskId)) return;
      const home = homes.get(fact.taskId);
      if (!home) {
        if (fact.fromDispatch) homes.set(fact.taskId, { viewIndex, factIndex });
        return;
      }
      const at = positionKey(viewIndex, factIndex);
      const current = views[home.viewIndex].taskResults[home.factIndex];
      if (current.id === fact.id) {
        // 同一张 relay 被逐轮重推时又派生了一遍：它已经折进主卡了，这份重复直接丢掉。
        dropped.add(at);
        return;
      }
      const merged = foldedTaskCard(current, fact);
      if (!merged) return;
      folded.add(fact.taskId);
      replacements.set(positionKey(home.viewIndex, home.factIndex), merged);
      dropped.add(at);
    });
  });
  // 没有 replacement、只有 drop 也要重建（重复的 relay 卡要消失）。
  if (replacements.size === 0 && dropped.size === 0) return views;
  return views.map((view, viewIndex) => {
    const taskResults: TaskResultFact[] = [];
    let changed = false;
    view.taskResults.forEach((fact, factIndex) => {
      const at = positionKey(viewIndex, factIndex);
      const replacement = replacements.get(at);
      if (replacement) {
        taskResults.push(replacement);
        changed = true;
        return;
      }
      if (dropped.has(at)) {
        changed = true;
        return;
      }
      taskResults.push(fact);
    });
    return changed ? { ...view, taskResults } : view;
  });
}
