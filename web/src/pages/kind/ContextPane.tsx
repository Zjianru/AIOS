/**
 * Kind 右栏「Kind 现在带着」（experience-spec-v1.md §6.2，brief §四.1 要求它占聚焦下面的剩余高度）。
 *
 * 只讲上下文由什么组成：每块一行 + 估算大小（≈、淡色虚线）、合计是服务商报的实际数；
 * 点开看具体每一项。角色「让他离开」、资料「拿掉」是 §6.2 允许的就地调整，带撤销。
 * 记忆「更正」要跳记忆页（第 3 期），这一版只读——写进第 2 期取舍，不在这里假做。
 */

import { useEffect, useMemo, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import { getKindContext } from '@/data/api';
import type { KindContextBlock, KindContextFixture, KindContextItem } from '@/data/fixtures/context';
import { cn } from '@/lib/utils';

function formatTokens(tokens: number): string {
  return tokens >= 1000 ? (tokens / 1000).toFixed(1) + 'k' : String(tokens);
}

function itemKey(block: KindContextBlock, item: KindContextItem): string {
  return block.key + ':' + item.id;
}

export function ContextPane() {
  const [data, setData] = useState<KindContextFixture | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [removed, setRemoved] = useState<Set<string>>(new Set());

  useEffect(() => {
    let alive = true;
    void (async () => {
      const next = await getKindContext();
      if (alive) setData(next);
    })();
    return () => {
      alive = false;
    };
  }, []);

  const live = useMemo(() => {
    if (!data) return null;
    const blocks = data.blocks.map((block) => {
      const items = block.items.filter((item) => !removed.has(itemKey(block, item)));
      const tokens = items.reduce((sum, item) => sum + item.tokens, 0);
      return { block, items, tokens, count: block.count ?? items.length };
    });
    const total = blocks.reduce((sum, entry) => sum + entry.tokens, 0);
    return { blocks, total, changed: removed.size > 0, ratioBase: Math.max(removed.size > 0 ? total : data.reportedTotal, 1) };
  }, [data, removed]);

  if (!data || !live) {
    return (
      <section className="flex min-h-0 flex-1 flex-col overflow-hidden" data-block="carrying">
        <div className="px-4 py-3 text-2 text-ink-3">正在取「Kind 现在带着」…</div>
      </section>
    );
  }

  function toggle(key: string): void {
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  function remove(block: KindContextBlock, item: KindContextItem): void {
    setRemoved((prev) => new Set(prev).add(itemKey(block, item)));
    toast((item.action === 'leave' ? item.label + '已离开' : item.label + ' 已拿掉'), {
      action: {
        label: '撤销',
        onClick: () =>
          setRemoved((prev) => {
            const next = new Set(prev);
            next.delete(itemKey(block, item));
            return next;
          }),
      },
    });
  }

  const totalLabel = live.changed ? '≈ ' + formatTokens(live.total) : formatTokens(data.reportedTotal);

  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden" data-block="carrying">
      <header className="shrink-0 px-4 pt-3">
        <div className="flex items-baseline gap-2">
          <h3 className="text-3 font-medium text-ink">Kind 现在带着</h3>
          <span className="ml-auto border-b border-dashed border-ink-3/40 text-2 text-ink-2 brand-type">
            {totalLabel}
          </span>
        </div>
        {/* 比例条分色：每块一个颜色（v13 的 --kf-c1..8），只画比例、不写占用率 */}
        <div className="mt-2 flex h-2 gap-[2px] overflow-hidden rounded-badge">
          {live.blocks.map((entry, index) =>
            entry.tokens > 0 ? (
              <span
                key={entry.block.key}
                title={entry.block.name + ' ≈ ' + formatTokens(entry.tokens)}
                style={{
                  width: (entry.tokens / live.ratioBase) * 100 + '%',
                  backgroundColor: 'var(--kf-c' + (index + 1) + ')',
                }}
              />
            ) : null,
          )}
        </div>
        {live.changed ? <p className="mt-1 text-1 text-ink-3">发出下一条消息后换成实际数</p> : null}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 pt-1.5">
        {live.blocks.map((entry, index) => {
          const isOpen = open.has(entry.block.key);
          const expandable = entry.block.items.length > 0;
          return (
            <div key={entry.block.key} className="border-b border-line/60 last:border-0">
              <button
                type="button"
                disabled={!expandable}
                onClick={() => toggle(entry.block.key)}
                className={cn(
                  'flex w-full items-center gap-1.5 py-1.5 text-left',
                  expandable && 'transition-colors hover:text-ink-2',
                )}
                title={expandable ? '展开看具体每一项' : undefined}
              >
                {expandable ? (
                  <ChevronRight
                    className={cn('size-3 shrink-0 text-ink-3 transition-transform', isOpen && 'rotate-90')}
                  />
                ) : (
                  <span className="size-3 shrink-0" />
                )}
                <span
                  className="size-[9px] shrink-0 rounded-[2px]"
                  style={{ backgroundColor: 'var(--kf-c' + (index + 1) + ')' }}
                  aria-hidden="true"
                />
                <span className="truncate text-2 text-ink">{entry.block.name}</span>
                {entry.block.unit ? (
                  <span className="shrink-0 text-1 text-ink-3 brand-type">
                    {entry.count} {entry.block.unit}
                  </span>
                ) : null}
                <span className="ml-auto shrink-0 border-b border-dashed border-ink-3/40 text-1 text-ink-3 brand-type">
                  {entry.tokens === 0 ? '—' : '≈ ' + formatTokens(entry.tokens)}
                </span>
              </button>
              {isOpen ? (
                <ul className="pb-1.5 pl-5 pr-1">
                  {entry.items.map((item) => (
                    <li key={item.id} className="group/item flex items-baseline gap-2 py-0.5 text-1">
                      <span className="min-w-0 flex-1 truncate text-ink-3" title={item.label}>
                        · {item.label}
                      </span>
                      <span className="shrink-0 brand-type text-ink-3 opacity-70">{formatTokens(item.tokens)}</span>
                      {item.action ? (
                        <button
                          type="button"
                          onClick={() => remove(entry.block, item)}
                          className="shrink-0 text-1 text-ink-3 opacity-0 transition-opacity hover:text-ink-2 group-hover/item:opacity-100"
                        >
                          {item.action === 'leave' ? '让他离开' : '拿掉'}
                        </button>
                      ) : null}
                    </li>
                  ))}
                  {entry.block.note ? (
                    <li className="pt-0.5 text-1 text-ink-3 opacity-80">{entry.block.note}</li>
                  ) : null}
                </ul>
              ) : null}
            </div>
          );
        })}
      </div>

      <footer className="shrink-0 border-t border-line px-4 py-2 text-1 text-ink-3">
        各块大小是估算（按每 4 个字约 1 token），合计是服务商报的实际数。
      </footer>
    </section>
  );
}
