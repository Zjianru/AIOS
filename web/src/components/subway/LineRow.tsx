/**
 * 一条横排的地铁线（app-v14 第 2 期）：
 * - 站 = 中性卡片：状态点只在站的上角，线色只留给连接线（brief §四.2 的通道分工）；
 * - 站名 13px、副标 11px；放不下就横向滚动，不缩字；
 * - 能同时进行的站不画箭头，有依赖的站之间画「→」；
 * - 现在页的卡片用它；工作页/任务页 C 段沿用同一套站与连线画法（届时加竖排与分叉）。
 */

import type { LineStationView } from '@/data/fixtures/now';
import type { TaskState } from '@/data/fixtures/workspace';
import { cn } from '@/lib/utils';

export interface LineRowProps {
  color: string;
  stations: LineStationView[];
  onGo?: (href: string) => void;
  className?: string;
}

const DOT: Record<TaskState, string> = {
  done: 'var(--done)',
  run: 'var(--run)',
  wait: 'var(--wait)',
  fail: 'var(--fail)',
  todo: 'var(--ink-3)',
};

export function LineRow({ color, stations, onGo, className }: LineRowProps) {
  return (
    <div className={cn('flex min-w-0 items-stretch gap-1 overflow-x-auto pb-0.5', className)} data-line-row="true">
      {stations.map((station, index) => (
        <div key={station.id} className="flex min-w-0 shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => station.href && onGo?.(station.href)}
            className="relative min-w-[118px] max-w-[190px] rounded-info border border-line-2 bg-pane px-2.5 py-1.5 text-left transition-colors hover:border-ink-3/50 hover:bg-raised"
            title={station.title}
          >
            <span
              className="absolute right-1.5 top-1.5 size-[7px] rounded-full"
              style={{ backgroundColor: DOT[station.state] }}
            />
            <span className="block truncate pr-3 text-3 leading-4 text-ink">{station.title}</span>
            <span className="mt-0.5 block truncate text-1 leading-3.5 text-ink-3">
              {station.role ? station.role : ''}
              {station.role && station.meta ? ' · ' : ''}
              {station.meta ?? ''}
            </span>
            {station.progress ? (
              <span className="mt-1 block h-[3px] w-full overflow-hidden rounded-full bg-hover">
                <span
                  className="block h-full rounded-full"
                  style={{ width: (station.progress.done / Math.max(station.progress.total, 1)) * 100 + '%', backgroundColor: color }}
                />
              </span>
            ) : null}
          </button>
          {index < stations.length - 1 ? (
            <span className="shrink-0 select-none text-1 leading-none text-ink-3" aria-hidden="true">
              →
            </span>
          ) : null}
        </div>
      ))}
    </div>
  );
}
