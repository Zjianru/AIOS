/**
 * T051：任务对话输入框下的任务级统计条（外观沿用 Kind 的 pill，不另设计）。
 *
 * 数字全部来自服务端 task-read 的 stats——和 Kind status 是同一个 session_stats 函数算的
 * （任务冻结的模型、当前 Attempt 全部事件、用户价/目录标价都在服务端算好）；页面只显示，
 * 不再自己数事件、不按模型显示名去设置目录匹配价格/窗口。多 Attempt 标次数。
 */
import { TooltipProvider } from '@/components/ui/tooltip';
import type { FxInfo } from '@/data/generated/FxInfo';
import type { SessionStats } from '@/data/generated/SessionStats';
import type { UsageTotals } from '@/data/turnUsage';
import { formatExactTokens, formatPercent, type Currency } from '@/lib/numbers';
import { cn } from '@/lib/utils';
import {
  ActivityPill,
  CostPill,
  Hint,
  Row,
  Ring,
  UsagePill,
  meterTone,
  pill,
} from '@/pages/kind/ComposerDock';

export function TaskStatsDock({
  stats,
  fx,
  currency,
  attempts,
}: {
  /** 服务端 task-read 给的当前 Attempt 统计（和 Kind status 同一函数）。 */
  stats: SessionStats;
  /** USD→CNY 汇率（服务端给；缺失时按 7.2 兜底，和 Kind 一样）。 */
  fx: FxInfo | null;
  /** 币种（settings.json 的 usd/cny）。 */
  currency: string | null;
  attempts: number;
}) {
  const usage: UsageTotals = {
    input: stats.inputTokens,
    output: stats.outputTokens,
    cacheRead: stats.cacheReadTokens,
    cacheWrite: stats.cacheWriteTokens,
  };
  const unit: Currency = currency === 'cny' ? 'cny' : 'usd';
  const rate = fx?.rate ?? 7.2;
  const percent =
    stats.contextWindow > 0 ? (stats.contextUsed / stats.contextWindow) * 100 : null;
  return (
    <TooltipProvider delayDuration={150}>
      <div className="mt-1.5 flex items-center gap-1 text-1" data-kind="task-stats">
        <ActivityPill turns={stats.turns} steps={stats.steps} />
        <UsagePill usage={usage} />
        <CostPill
          cost={stats.costUsd ?? null}
          unpriced={stats.unpriced}
          currency={unit}
          cnyPerUsd={rate}
          fxAt={fx?.fetchedAt}
        />
        {percent === null ? null : (
          <Hint
            content={
              <>
                <Row label="已用（这次调用）" value={formatExactTokens(stats.contextUsed)} />
                <Row label="窗口" value={formatExactTokens(stats.contextWindow)} />
              </>
            }
          >
            <span
              className={cn(pill, meterTone(Math.round(percent)))}
              data-kind="context-meter"
            >
              <Ring percent={Math.round(percent)} />
              <span data-kind="stat-number" className="tabular-nums">
                {formatPercent(percent)}
              </span>
            </span>
          </Hint>
        )}
        {attempts > 1 ? (
          <span className={pill} data-kind="task-attempt-stat">
            第 {attempts} 次 Attempt
          </span>
        ) : null}
        <span className="ml-auto" />
      </div>
    </TooltipProvider>
  );
}
