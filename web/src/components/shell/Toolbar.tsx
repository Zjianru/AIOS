/**
 * 工具栏：固定 44px，放面包屑与侧栏开关（连接视图只剩 Kind 一页）。
 */

import { ChevronRight } from 'lucide-react';
import { SidebarTrigger } from '@/components/ui/sidebar';
import type { ParsedRoute } from '@/app/route';
import { crumbIdOf, crumbOf } from '@/app/route';

interface ToolbarProps {
  route: ParsedRoute;
}

export function Toolbar({ route }: ToolbarProps) {
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
