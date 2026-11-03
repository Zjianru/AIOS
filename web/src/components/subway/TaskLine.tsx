/**
 * 任务页的横向地铁线（§4.1；brief §四.2 的通道分工）：
 * - 能同时进行的站叠在同一列；列与列之间的箭头表示「前面的完成后才能开始后面的」；
 * - 每站显示：状态点（只在站角）、名称、谁在做；有子任务再显示「2 / 3 项」和小进度条；
 * - 点一站进入下一层（同一个组件再画一遍）。
 * 画法是中性描边 + 线色只做强调，不做整站染色。
 */

import { go } from '@/app/navigate';
import type { TaskDetail } from '@/data/selectors';
import type { TaskState } from '@/data/fixtures/workspace';
import { cn } from '@/lib/utils';

const DOT: Record<TaskState, string> = {
  done: 'var(--done)',
  run: 'var(--run)',
  wait: 'var(--wait)',
  fail: 'var(--fail)',
  todo: 'var(--ink-3)',
};

export function TaskLine({ detail }: { detail: TaskDetail }) {
  const ids = new Set(detail.children.map((child) => child.id));
  const depth = new Map<string, number>();

  const computeDepth = (id: string): number => {
    const cached = depth.get(id);
    if (cached !== undefined) return cached;
    const child = detail.children.find((item) => item.id === id);
    if (!child) return 0;
    depth.set(id, 0);
    const deps = child.deps.filter((dep) => ids.has(dep));
    const value = deps.length ? Math.max(...deps.map((dep) => computeDepth(dep))) + 1 : 0;
    depth.set(id, value);
    return value;
  };
  detail.children.forEach((child) => computeDepth(child.id));

  const maxDepth = Math.max(0, ...detail.children.map((child) => depth.get(child.id) ?? 0));
  const columns = Array.from({ length: maxDepth + 1 }, (_, index) =>
    detail.children.filter((child) => (depth.get(child.id) ?? 0) === index),
  );

  return (
    <div className="flex min-w-0 items-start gap-2 overflow-x-auto pb-1" data-task-line="true">
      {columns.map((column, columnIndex) => (
        <div key={columnIndex} className="flex items-center gap-2">
          <div className="flex min-w-[210px] flex-col gap-2">
            {column.map((child) => (
              <button
                key={child.id}
                type="button"
                onClick={() => go('task:' + child.id)}
                className={cn(
                  'rounded-card border bg-pane px-3 py-2 text-left transition-colors hover:bg-raised',
                  child.state === 'wait' ? 'border-wait/45' : child.state === 'fail' ? 'border-fail/45' : 'border-line-2',
                )}
              >
                <span className="flex min-w-0 items-center gap-1.5">
                  <span className="size-[7px] shrink-0 rounded-full" style={{ backgroundColor: DOT[child.state] }} />
                  <span className="truncate text-3 text-ink">{child.title}</span>
                </span>
                <span className="mt-0.5 block truncate text-1 text-ink-3">
                  {child.role || '—'}
                  {child.note ? ' · ' + child.note : ''}
                </span>
                {child.progress ? (
                  <>
                    <span className="mt-0.5 block text-1 text-ink-3">
                      {child.progress.done} / {child.progress.total} 项
                    </span>
                    <span className="mt-1 block h-[3px] w-full overflow-hidden rounded-full bg-hover">
                      <span
                        className="block h-full rounded-full"
                        style={{
                          width: (child.progress.done / Math.max(child.progress.total, 1)) * 100 + '%',
                          backgroundColor: detail.color,
                        }}
                      />
                    </span>
                  </>
                ) : null}
              </button>
            ))}
          </div>
          {columnIndex < columns.length - 1 ? (
            <span className="shrink-0 select-none text-1 text-ink-3" aria-hidden="true">
              →
            </span>
          ) : null}
        </div>
      ))}
    </div>
  );
}
