/**
 * T043：任务对话输入框下的任务级统计条（外观沿用 Kind 的 pill，不另设计）。
 *
 * 只统计当前任务当前 Attempt：轮/步/token 来自 task-read 的事件聚合；价格/窗口按
 * 任务头的模型显示名去设置目录里可靠匹配，匹配不到就隐藏（不猜）。多 Attempt 标次数。
 */
import { useEffect, useState } from 'react';

import { TooltipProvider } from '@/components/ui/tooltip';
import { getModelSettings } from '@/data/keyring';
import type { ModelSettingsView } from '@/data/keyring';
import type { UsageTotals } from '@/data/turnUsage';
import { formatExactTokens, formatPercent } from '@/lib/numbers';
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
import { modelFactsOf } from './taskStats';

export function TaskStatsDock({
  turns,
  steps,
  usage,
  model,
  contextUsed,
  attempts,
}: {
  turns: number;
  steps: number;
  usage: UsageTotals;
  /** 任务头冻结的模型显示名（只给显示名，价格/窗口从这里匹配）。 */
  model: string;
  contextUsed: number | null;
  attempts: number;
}) {
  const [settings, setSettings] = useState<ModelSettingsView | null>(null);
  useEffect(() => {
    let alive = true;
    void getModelSettings()
      .then((view) => {
        if (alive) setSettings(view);
      })
      .catch(() => {
        // 设置读不到：价格/窗口没有可靠来源，隐藏对应项，其余统计照常。
        if (alive) setSettings(null);
      });
    return () => {
      alive = false;
    };
  }, [model]);
  const facts = modelFactsOf(model, settings, usage, contextUsed);
  const percent =
    facts.contextWindow !== null && facts.contextUsed !== null
      ? (facts.contextUsed / facts.contextWindow) * 100
      : null;
  return (
    <TooltipProvider delayDuration={150}>
      <div className="mt-1.5 flex items-center gap-1 text-1" data-kind="task-stats">
        <ActivityPill turns={turns} steps={steps} />
        <UsagePill usage={usage} />
        <CostPill cost={facts.cost} unpriced={false} />
        {percent === null || facts.contextUsed === null || facts.contextWindow === null ? null : (
          <Hint
            content={
              <>
                <Row label="已用（这次调用）" value={formatExactTokens(facts.contextUsed)} />
                <Row label={'窗口'} value={formatExactTokens(facts.contextWindow)} />
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
