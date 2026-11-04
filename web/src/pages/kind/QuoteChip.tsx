/**
 * 引用片（v10-tables §1）：`[图标 名字 › 位置]`，嵌在文字里；你的话和 Kind 的话同一种画法。
 * 颜色只按类型分；停上去能预览，点一下跳过去。
 */

import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { go } from '@/app/navigate';
import type { QuoteKind, TextElement } from '@/data/types';
import { cn } from '@/lib/utils';

const GLYPH: Record<QuoteKind, string> = {
  role: '@',
  capability: '$',
  material: '▤',
  artifact: '▤',
  task: '◐',
  project: '■',
  pin: '📍',
};

const TONE: Record<QuoteKind, string> = {
  role: 'text-line-w',
  capability: 'text-line-n',
  material: 'text-line-d',
  artifact: 'text-line-d',
  task: 'text-run',
  project: 'text-line-w',
  pin: 'text-wait',
};

const LABEL: Record<QuoteKind, string> = {
  role: '角色',
  capability: '能力',
  material: '资料',
  artifact: '成果',
  task: '任务',
  project: '项目',
  pin: '批注位置',
};

export function QuoteChip({ element }: { element: TextElement }) {
  const glyph = GLYPH[element.kind];
  // label 里如果已经带了前缀符号，就不再重复画一遍
  const text = element.label.startsWith(glyph) ? element.label.slice(glyph.length).trim() : element.label;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={() => element.target && go(element.target.replace(/^#/, ''))}
          className={cn(
            'mx-[1px] inline-flex max-w-full items-baseline gap-1 rounded-badge px-1 py-[1px] align-baseline',
            'bg-hover/60 text-[0.96em] leading-[1.5] transition-colors hover:bg-active',
          )}
        >
          <span className={cn('shrink-0 select-none', TONE[element.kind])}>{glyph}</span>
          <span className="truncate text-ink-2">{text}</span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-[280px] text-2">
        <div className="text-ink-3">{LABEL[element.kind]}</div>
        <div>{element.label}</div>
      </TooltipContent>
    </Tooltip>
  );
}
