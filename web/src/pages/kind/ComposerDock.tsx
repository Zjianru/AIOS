/**
 * 输入框下面的一行（照 DSH 的 composer dock、原型 app-v14 1f0c920）：
 * - 「N 轮 · M 步」；
 * - 「↑ 输入 ↓ 输出 · 缓存命中 %」：数字自动换单位，悬停给精确值；
 * - 花费：有价格才出现，跟着设置里的币种（没有价格就不显示，不按 0 算）；
 * - 右边一个上下文圆环：占用 %，点开看用了多少、窗口多大，在这里压缩。
 *
 * T012.9：数据来自服务 status.stats（内核从日志算好推过来；页面不自己加）。
 */

import type { ReactNode } from 'react';
import { ArrowDown, ArrowUp, CircleHelp, Gauge, LoaderCircle, Wallet } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { formatCacheHitPercent, formatCost, formatExactTokens, formatFxTime, formatPercent, formatTokens } from '@/lib/numbers';
import { cn } from '@/lib/utils';
import { useKindStore } from './kindStore';

/** 浅色的悬停卡片：和页面同一套底色、描边、阴影。 */
export function Hint({
  content,
  children,
  side = 'top',
}: {
  content: ReactNode;
  children: ReactNode;
  side?: 'top' | 'bottom';
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent
        side={side}
        sideOffset={6}
        arrow={false}
        className="min-w-[180px] rounded-card border border-line-2 bg-raised px-3 py-2 text-2 text-ink-2 shadow-[var(--shadow-pop)]"
      >
        {content}
      </TooltipContent>
    </Tooltip>
  );
}

/** 悬停卡片里的一行：左边名字，右边数字（等宽对齐）。 */
function Row({ label, value, strong }: { label: string; value: string; strong?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-6 py-[1px]">
      <span className={strong ? 'text-ink' : 'text-ink-3'}>{label}</span>
      <span className={cn('brand-type tabular-nums', strong ? 'text-ink' : 'text-ink-2')}>{value}</span>
    </div>
  );
}

const pill =
  'flex h-6 items-center gap-1.5 rounded-full px-2 text-ink-3 tabular-nums transition-colors hover:bg-hover hover:text-ink-2';

function Dot() {
  return <span className="mx-0.5 opacity-50">·</span>;
}

function ActivityPill() {
  const turns = useKindStore((state) => state.stats.turns);
  const steps = useKindStore((state) => state.stats.steps);
  return (
    <Hint
      content={
        <>
          <Row label="轮" value={formatExactTokens(turns)} />
          <Row label="步" value={formatExactTokens(steps)} />
          <p className="mt-1.5 text-1 text-ink-3">
            你发一条算一轮；模型每回一次算一步，一轮里调了几次工具就多几步。
          </p>
        </>
      }
    >
      <span className={pill} data-kind="stats-activity">
        <Gauge className="size-3.5" />
        <span className="brand-type">{formatExactTokens(turns)}</span> 轮
        <Dot />
        <span className="brand-type">{formatExactTokens(steps)}</span> 步
      </span>
    </Hint>
  );
}

function UsagePill() {
  const stats = useKindStore((state) => state.stats);
  const prompt = stats.input + stats.cacheRead + stats.cacheWrite;
  // 没命中过缓存（0）就不显示「缓存命中 0%」；命中过才显示。
  const hit = stats.cacheRead > 0 ? formatCacheHitPercent(stats.cacheRead, prompt) : null;
  return (
    <Hint
      content={
        <>
          <Row label="输入（没命中缓存）" value={formatExactTokens(stats.input)} />
          <Row label="输入（命中缓存）" value={formatExactTokens(stats.cacheRead)} />
          {stats.cacheWrite > 0 ? <Row label="写入缓存" value={formatExactTokens(stats.cacheWrite)} /> : null}
          <Row label="输出" value={formatExactTokens(stats.output)} />
          <div className="my-1 border-t border-line" />
          <Row label="合计" value={formatExactTokens(prompt + stats.output)} strong />
          {hit === null ? null : <Row label="缓存命中" value={hit} />}
        </>
      }
    >
      <span className={pill} data-kind="stats-usage">
        <ArrowUp className="size-3" />
        <span className="brand-type">{formatTokens(prompt)}</span>
        <ArrowDown className="ml-0.5 size-3" />
        <span className="brand-type">{formatTokens(stats.output)}</span>
        {hit === null ? null : (
          <>
            <Dot />
            缓存命中 <span className="brand-type">{hit}</span>
          </>
        )}
      </span>
    </Hint>
  );
}

function CostPill() {
  const stats = useKindStore((state) => state.stats);
  const money = useKindStore((state) => state.money);
  // 一个价格都没有：不显示花费（不按 0 算）。
  if (stats.cost === null) return null;
  return (
    <Hint
      content={
        <>
          <Row label="这个会话的花费" value={formatCost(stats.cost, money.currency, money.cnyPerUsd, stats.unpriced)} strong />
          <p className="mt-1.5 max-w-[240px] text-1 leading-4 text-ink-3">
            按每个模型的价格算（设置 › 模型与连接）。
            {stats.unpriced ? '有的轮用的模型没有价格，没算进来，所以后面带「+」。' : null}
            {money.currency === 'cny'
              ? ' 价格按美元记，这里按 1 美元 = ' +
                money.cnyPerUsd +
                ' 元换算' +
                (formatFxTime(money.fxAt) ? '（' + formatFxTime(money.fxAt) + '的汇率）' : '（没取到汇率，先按 7.2）') +
                '。'
              : null}
          </p>
        </>
      }
    >
      <span className={pill} data-kind="stats-cost">
        <Wallet className="size-3.5" />
        <span className="brand-type">{formatCost(stats.cost, money.currency, money.cnyPerUsd, stats.unpriced)}</span>
      </span>
    </Hint>
  );
}

/** 占用到多少换颜色：70% 起提醒，90% 起要压缩了。 */
function meterTone(percent: number): string {
  if (percent >= 90) return 'text-fail';
  if (percent >= 70) return 'text-wait';
  return 'text-ink-2';
}

/** 上下文圆环（照 DSH ContextMeter：14px 的环，按百分比描一段，从 12 点钟方向开始）。 */
function Ring({ percent, className }: { percent: number; className?: string }) {
  const r = 5.5;
  const length = 2 * Math.PI * r;
  return (
    <svg viewBox="0 0 14 14" className={cn('size-3.5 -rotate-90', className)} aria-hidden>
      <circle cx="7" cy="7" r={r} fill="none" stroke="currentColor" strokeOpacity={0.2} strokeWidth={2} />
      <circle
        cx="7"
        cy="7"
        r={r}
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeDasharray={`${(Math.min(100, percent) / 100) * length} ${length}`}
      />
    </svg>
  );
}

function ContextMeter({ running }: { running: boolean }) {
  // T012.9：占用/窗口都来自服务 status.stats（内核算好），页面不自己算。
  const stats = useKindStore((state) => state.stats);
  const compacting = useKindStore((state) => state.compacting);
  const compact = useKindStore((state) => state.compact);
  const label = useKindStore((state) => state.model.label);
  const window = stats.contextWindow > 0 ? stats.contextWindow : undefined;
  const used = stats.contextUsed > 0 ? stats.contextUsed : null;
  // T013.6：占用 >0 但不到 1% 显示「不到 1%」；圆环/颜色仍用四舍五入后的数字。
  const usageValue = stats.contextWindow > 0 ? (stats.contextUsed / stats.contextWindow) * 100 : 0;
  const usage = Math.round(usageValue);
  const usageText = formatPercent(usageValue);

  if (compacting) {
    return (
      <span className={cn(pill, 'text-ink-2')} data-kind="compacting">
        <LoaderCircle className="size-3.5 animate-spin" />
        正在压缩上下文…
      </span>
    );
  }
  return (
    <Popover>
      <Hint
        content={
          <>
            上下文已用 <span className="brand-type">{usageText}</span>
            {used !== null ? (
              <>
                {' '}
                · 约 <span className="brand-type">{formatTokens(used)}</span> /{' '}
                <span className="brand-type">{formatTokens(window!)}</span>
              </>
            ) : null}
          </>
        }
      >
        <PopoverTrigger asChild>
          <button type="button" className={cn(pill, meterTone(usage))} data-kind="context-meter">
            <Ring percent={usage} />
            <span className="brand-type">{usageText}</span>
          </button>
        </PopoverTrigger>
      </Hint>
      <PopoverContent
        side="top"
        align="end"
        sideOffset={8}
        onCloseAutoFocus={(event) => event.preventDefault()}
        className="w-64 rounded-card border-line-2 bg-raised p-3 text-2 shadow-[var(--shadow-pop)]"
      >
        <div className="flex items-baseline justify-between">
          <span className="text-ink">上下文</span>
          <span className={cn('brand-type', meterTone(usage))}>{usageText}</span>
        </div>
        <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-hover">
          <div
            className={cn('h-full rounded-full', usage >= 90 ? 'bg-fail' : usage >= 70 ? 'bg-wait' : 'bg-ink-3')}
            style={{ width: usage + '%' }}
          />
        </div>
        <div className="mt-2 space-y-[1px]">
          {used !== null ? <Row label="已用（约）" value={formatExactTokens(used)} /> : null}
          {window ? <Row label={label + ' 的窗口'} value={formatExactTokens(window)} /> : <Row label="窗口" value="不知道" />}
        </div>
        <p className="mt-2 text-1 leading-4 text-ink-3">
          快满时会自动压缩。也可以现在压：之前的对话换成一份摘要，原文留在日志里。
        </p>
        <button
          type="button"
          onClick={compact}
          disabled={running || usage === 0}
          title={running ? '这一轮做完再压缩' : undefined}
          className="mt-2 w-full rounded-btn border border-line-2 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
          data-kind="compact"
        >
          {running ? '这一轮做完再压缩' : '现在压缩'}
        </button>
      </PopoverContent>
    </Popover>
  );
}

export function ComposerDock({
  running,
  waitingQuestion,
  onJumpToQuestion,
}: {
  running: boolean;
  waitingQuestion: boolean;
  onJumpToQuestion: () => void;
}) {
  return (
    <TooltipProvider delayDuration={150}>
      <div className="mt-1.5 flex items-center gap-1 text-1" data-kind="composer-dock">
        <ActivityPill />
        <UsagePill />
        <CostPill />
        <span className="ml-auto" />
        {waitingQuestion ? (
          <button
            type="button"
            onClick={onJumpToQuestion}
            className="flex items-center gap-1 px-2 text-wait transition-opacity hover:opacity-80"
            data-kind="waiting-hint"
          >
            <CircleHelp className="size-3" />
            Kind 在等你回答
          </button>
        ) : null}
        <ContextMeter running={running} />
      </div>
    </TooltipProvider>
  );
}
