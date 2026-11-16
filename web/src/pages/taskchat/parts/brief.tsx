//! 简报：标题、要求、DoD、冻结的模型与工具。

import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { cn } from '@/lib/utils';
import { toolsText } from './helpers';

export function Brief({ task }: { task: TaskReadHeader }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-2">
      <button
        type="button"
        data-kind="task-brief-toggle"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
        任务要求和 DoD
      </button>
      {open ? (
        <div
          className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-l border-line pl-3 text-2 leading-5"
          data-kind="task-brief"
        >
          <span className="text-ink-3">要求</span>
          <span className="text-ink-2">{task.requirement}</span>
          <span className="text-ink-3">DoD</span>
          <span className="text-ink-2">{task.dod}</span>
          <span className="text-ink-3">模型</span>
          <span className="text-ink-2">{task.model}（派出时定下，不跟着改）</span>
          <span className="text-ink-3">工具</span>
          <span className="text-ink-2">{toolsText(task.tools)}</span>
          <span className="text-ink-3">Attempt</span>
          <span className="text-ink-2">第 {task.attempts} 次</span>
        </div>
      ) : null}
    </div>
  );
}
