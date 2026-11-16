//! 菜单与返回：停止入口、回 Kind。

import { ArrowLeft, ChevronDown } from 'lucide-react';
import { go } from '@/app/navigate';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';

export function TaskMenu({
  running,
  winding,
  program,
  onStopNow,
  onStopAfterReport,
}: {
  running: boolean;
  winding: boolean;
  program: boolean;
  onStopNow: () => void;
  onStopAfterReport: () => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="mt-[1px] flex shrink-0 items-center gap-0.5 rounded-btn border border-line-2 px-2 py-0.5 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
          data-kind="task-menu"
        >
          任务
          <ChevronDown className="size-3" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-[280px]">
        <DropdownMenuItem
          disabled={!running}
          onSelect={onStopNow}
          className="flex-col items-start gap-0"
        >
          <span>立即停</span>
          <span className="text-1 text-ink-3">马上掐断，这次 Attempt 记成已停止</span>
        </DropdownMenuItem>
        {program ? null : (
        <DropdownMenuItem
          disabled={!running || winding}
          onSelect={onStopAfterReport}
          className="flex-col items-start gap-0"
        >
          <span>收尾停</span>
          <span className="text-1 text-ink-3">做完手上这一步、交一份报告就结束</span>
        </DropdownMenuItem>
        )}
        {running ? null : <div className="px-2 py-1.5 text-1 text-ink-3">现在没在跑。</div>}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}


export function BackToKind() {
  return (
    <button
      type="button"
      onClick={() => go('kind')}
      className="-ml-1 flex items-center gap-1 rounded-btn px-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
    >
      <ArrowLeft className="size-3.5" />
      回 Kind 对话
    </button>
  );
}
