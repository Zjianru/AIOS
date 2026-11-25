//! 状态徽标：在跑/收尾/停了/被打断/报告。

import { Hourglass, PlugZap, Square } from 'lucide-react';
import type { TaskReadReport } from '@/data/generated/TaskReadReport';
import type { TaskReadUsageWait } from '@/data/generated/TaskReadUsageWait';
import { taskStatusOf } from '@/data/taskResult';

export function TaskState({
  running,
  winding,
  stopped,
  interrupted,
  report,
  review,
  accepted,
  usageWait,
}: {
  running: boolean;
  winding: boolean;
  stopped: boolean;
  interrupted: boolean;
 /** 当前 Attempt 最近一份报告的 { state, origin }；null = 还没交报告。 */
  report: TaskReadReport | null;
 /** 写任务当前版本的 ready（和 Kind 卡「待你核」同一判断）。 */
  review: boolean;
 /** 用户接受记录绑的就是当前版本和这份报告。 */
  accepted: boolean;
 /** 正在等模型用量恢复；普通等待为 null。 */
  usageWait: TaskReadUsageWait | null;
}) {
  if (running) {
    return (
      <span
        className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-run-soft px-1.5 py-[1px] text-1 text-run"
        data-kind="task-state"
      >
        {winding ? '执行中 · 收尾' : '执行中'}
      </span>
    );
  }
  if (stopped) {
    return (
      <span
        className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-hover px-1.5 py-[1px] text-1 text-ink-2"
        data-kind="task-state"
      >
        <Square className="size-3 shrink-0" />
        已停止
      </span>
    );
  }
  if (interrupted) {
    return (
      <span
        className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-wait-soft px-1.5 py-[1px] text-1 text-wait"
        data-kind="task-state"
      >
        <PlugZap className="size-3 shrink-0" />
        被打断了
      </span>
    );
  }
  if (report !== null) {
    const word = taskStatusOf(
      report.state,
      report.origin,
      review ? 'review' : undefined,
      accepted,
      usageWait,
    );
    const failed = word === '失败';
    return (
      <span
        className={
          failed
            ? 'mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-fail-soft px-1.5 py-[1px] text-1 text-fail'
            : 'mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-wait-soft px-1.5 py-[1px] text-1 text-wait'
        }
        data-kind="task-state"
      >
        <Hourglass className="size-3 shrink-0" />
        {word}
      </span>
    );
  }
 // 兜底：任务已派出但还没开跑/在排队时也要有状态词，页头不空白。
 // note：词只从 taskStatusOf 一处出（pending 派发态就是「等待中」），这里不再写字面量。
  return (
    <span
      className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-hover px-1.5 py-[1px] text-1 text-ink-2"
      data-kind="task-state"
    >
      {taskStatusOf('pending', 'executor')}
    </span>
  );
}
