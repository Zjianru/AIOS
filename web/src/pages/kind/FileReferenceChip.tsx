/**
 * T013 验收 F6：消息里的 @文件引用片（docs/10 §11）。
 * 只显示路径，不附带文件内容、不自动读文件；悬停/聚焦才给完整路径。
 */

import { FileText } from 'lucide-react';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';

export function FileReferenceChip({ path }: { path: string }) {
  return (
    <TooltipProvider delayDuration={150}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            data-kind="file-reference"
            data-ref-path={path}
            title={path}
            tabIndex={0}
            className="mx-[1px] inline-flex max-w-[240px] items-baseline gap-1 rounded-badge bg-hover/60 px-1 py-[1px] align-baseline text-[0.96em] leading-[1.5] transition-colors hover:bg-active"
          >
            <FileText className="size-3 shrink-0 self-center text-ink-3" aria-hidden />
            <span className="truncate text-ink-2">{path}</span>
          </span>
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-[280px] text-2">
          <div className="text-ink-3">文件路径</div>
          <div className="break-all">{path}</div>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
