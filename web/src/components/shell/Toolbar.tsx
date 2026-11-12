/**
 * 工具栏：固定 44px，放面包屑与侧栏开关（连接视图只剩 Kind 一页）。
 */

import { ChevronRight } from 'lucide-react';
import { SidebarTrigger } from '@/components/ui/sidebar';
import { go } from '@/app/navigate';
import type { ParsedRoute } from '@/app/route';
import { crumbIdOf, crumbOf } from '@/app/route';
import { useTaskChromeStore } from '@/app/taskChrome';

interface ToolbarProps {
  route: ParsedRoute;
}

export function Toolbar({ route }: ToolbarProps) {
  const taskTitle = useTaskChromeStore((state) => state.taskTitle);
  if (route.kind === 'taskchat') {
    // 任务对话：面包屑是「任务对话 › 任务标题」（加载前只有第一段），整块可点回 Kind。
    return (
      <header className="flex h-11 shrink-0 items-center gap-1 border-b border-line px-2 text-3">
        <SidebarTrigger className="text-ink-3 hover:bg-hover hover:text-ink" />
        <button
          type="button"
          onClick={() => go('kind')}
          title="回 Kind 对话"
          data-kind="toolbar-back"
          className="ml-1 flex min-w-0 items-center gap-1 rounded-btn px-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink"
        >
          <span className="truncate">{crumbOf(route)}</span>
          {taskTitle ? (
            <>
              <ChevronRight className="size-3.5 shrink-0 opacity-60" />
              <span className="truncate text-ink">{taskTitle}</span>
            </>
          ) : null}
        </button>
      </header>
    );
  }
  return (
    <header className="flex h-11 shrink-0 items-center gap-1 border-b border-line px-2 text-3">
      <SidebarTrigger className="text-ink-3 hover:bg-hover hover:text-ink" />
      <div className="ml-1 flex min-w-0 items-center gap-1 text-ink-3">
        <span className="truncate text-ink">{crumbOf(route)}</span>
        {route.id ? (
          <>
            <ChevronRight className="size-3.5 shrink-0 opacity-60" />
            <span className={route.kind === 'settings' ? 'truncate' : 'truncate brand-type'}>{crumbIdOf(route)}</span>
          </>
        ) : null}
      </div>
    </header>
  );
}
