/**
 * Kind 右栏「Kind 现在带着」（T098.2）。
 *
 * 色条总长用现有 ContextUsage（used/window 真值）；块名照原型列。
 * 各块多少 token 内核没有这个数：照空状态写「还没有分块估算」，不编数字（reply s）。
 * 分块估算列后续（协调线排 T094 后）。
 */

import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { useKindStore } from './kindStore';
import { cn } from '@/lib/utils';

const BLOCKS = ['基本设定', '能力说明', '角色', '记忆', '资料', '你发的批注', '对话历史', '工具结果'];

function formatTokens(tokens: number): string {
  if (tokens >= 1000) return (tokens / 1000).toFixed(1) + 'k';
  return String(tokens);
}

export function ContextPane() {
  const usage = useKindStore((state) => state.usage);
  const window = useKindStore((state) => state.contextWindow);
  const [open, setOpen] = useState<Set<string>>(new Set());

  const toggle = (name: string) => {
    setOpen((previous) => {
      const next = new Set(previous);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const used = window > 0 ? Math.round((usage / 100) * window) : 0;

  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden" data-block="carrying">
      <header className="shrink-0 px-4 pt-3">
        <div className="flex items-baseline gap-2">
          <h3 className="text-3 font-medium text-ink">Kind 现在带着</h3>
          <span className="ml-auto border-b border-dashed border-ink-3/40 text-2 text-ink-2 brand-type">
            {window > 0 ? formatTokens(used) + ' / ' + formatTokens(window) : '—'}
          </span>
        </div>
        <div className="mt-2 h-2 overflow-hidden rounded-badge bg-line/60">
          <span className="block h-full bg-run transition-[width]" style={{ width: Math.max(1, usage) + '%' }} />
        </div>
        <p className="mt-1 text-1 text-ink-3">总长是服务商报的实际占用（used / window）。</p>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 pt-1.5">
        {BLOCKS.map((name, index) => {
          const isOpen = open.has(name);
          return (
            <div key={name} className="border-b border-line/60 last:border-0">
              <button
                type="button"
                data-context-block={name}
                onClick={() => toggle(name)}
                className="flex w-full items-center gap-1.5 py-1.5 text-left transition-colors hover:text-ink-2"
              >
                <ChevronRight className={cn('size-3 shrink-0 text-ink-3 transition-transform', isOpen && 'rotate-90')} />
                <span
                  className="size-[9px] shrink-0 rounded-[2px]"
                  style={{ backgroundColor: 'var(--kf-c' + (index + 1) + ')' }}
                  aria-hidden="true"
                />
                <span className="truncate text-2 text-ink">{name}</span>
                <span className="ml-auto shrink-0 text-1 text-ink-3">还没有分块估算</span>
              </button>
              {isOpen ? (
                <p className="pb-1.5 pl-5 pr-1 text-1 leading-4 text-ink-3">
                  内核还没有给出这一块的大小和条目；等分块估算接上再显示，不编数字。
                </p>
              ) : null}
            </div>
          );
        })}
        <p className="py-2 text-1 leading-4 text-ink-3">
          各块大小还没有估算；色条总长是服务商报的实际占用，不按估算补。
        </p>
      </div>
    </section>
  );
}
