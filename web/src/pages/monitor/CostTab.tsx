/**
 * 监控 · 消耗：范围、大数字、每天柱、钱/时间两个环、模型/工具/命令排行、任务卡。
 * Kind 只进合计，不产任务卡、不做诊断；项目只有「全部」，任务按未归项目显示。
 */

import { useMemo } from 'react';
import { ArrowUpRight, Sparkles } from 'lucide-react';
import { go } from '@/app/navigate';
import { formatTokens } from '@/lib/numbers';
import { cn } from '@/lib/utils';
import { useMonitorStore } from './monitorStore';
import type { MonitorRange, MonitorTaskSummary, MonitorUsageRow } from '@/data/service';
import { Card, Donut, Kpi, Missing, RankRow, Segmented, fmtClock } from './monitorParts';

const MODEL_COLOR = 'var(--kf-c1)';
const TOOL_COLOR = 'var(--kf-c4)';
const CMD_COLOR = 'var(--kf-c3)';
const KIND_COLOR = 'var(--kf-c5)';

export function formatUsd(value: number | null | undefined, partial = false): string {
  if (value == null) return '没采到';
  const text = value >= 100 ? value.toFixed(0) : value >= 1 ? value.toFixed(2) : value.toFixed(3);
  return '$' + text + (partial ? '+' : '');
}

function stateLabel(state: string): string {
  const labels: Record<string, string> = {
    needs_conditions: '待补条件',
    pending: '排队',
    running: '在跑',
    ready_for_review: '待检查',
    requirement_met: '已完成',
    closed: '已关闭',
  };
  return labels[state] ?? state;
}

function dayKey(ms: number): string {
  const date = new Date(ms);
  return date.getFullYear() + '-' + (date.getMonth() + 1) + '-' + date.getDate();
}

function DailyBars({ range, tasks }: { range: MonitorRange; tasks: MonitorTaskSummary[] }) {
  const days = useMemo(() => {
    const now = new Date();
    return Array.from({ length: 14 }, (_, index) => {
      const date = new Date(now.getFullYear(), now.getMonth(), now.getDate() - (13 - index));
      return {
        key: dayKey(date.getTime()),
        date,
        cost: 0,
        tokensIn: 0,
        tokensOut: 0,
        requests: 0,
        unpriced: 0,
      };
    });
  }, []);
  const byDay = new Map(days.map((day) => [day.key, day]));
  for (const task of tasks) {
    const at = task.endedAt ?? task.startedAt;
    if (!at) continue;
    const day = byDay.get(dayKey(at));
    if (!day) continue;
    day.cost += task.costUsd ?? 0;
    day.tokensIn += task.inputTokens;
    day.tokensOut += task.outputTokens;
    day.requests += task.requests;
    if (task.unpriced) day.unpriced += 1;
  }
  const max = Math.max(...days.map((day) => day.cost), 0.01);
  const top = Math.ceil(max * 2) / 2;
  const inRange = (index: number) => {
    if (range === 'today') return index === days.length - 1;
    if (range === 'last7-days') return index >= days.length - 7;
    return true;
  };
  return (
    <div data-kind="cost-daily">
      <div className="flex h-[168px] gap-2">
        <div className="brand-type flex w-12 shrink-0 flex-col justify-between pb-5 text-right text-1 text-ink-3">
          <span>{formatUsd(top)}</span>
          <span>{formatUsd(top / 2)}</span>
          <span>0</span>
        </div>
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 items-end gap-[4px] border-b border-line-2">
            {days.map((day, index) => {
              const dim = !inRange(index);
              return (
                <div key={day.key} className="relative flex h-full min-w-0 flex-1 flex-col justify-end" title={new Date(day.date).toLocaleDateString('zh-CN')}>
                  <div
                    className="relative w-full rounded-t-[4px]"
                    style={{
                      height: Math.max(day.cost ? 2 : 0, (day.cost / top) * 100) + '%',
                      background: dim ? 'var(--line-2)' : MODEL_COLOR,
                    }}
                  />
                  {day.unpriced && !dim ? (
                    <span className="brand-type absolute bottom-full left-1/2 mb-0.5 -translate-x-1/2 text-1 leading-none text-ink-3">
                      +
                    </span>
                  ) : null}
                </div>
              );
            })}
          </div>
          <div className="brand-type flex h-5 items-end gap-[4px] text-1 text-ink-3">
            {days.map((day, index) => (
              <span key={day.key} className="min-w-0 flex-1 text-center">
                {index % 4 === 0 || index === days.length - 1 ? new Date(day.date).getDate() : ''}
              </span>
            ))}
          </div>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 pl-14 text-1 text-ink-2">
        <span className="flex items-center gap-1.5">
          <i className="size-2 rounded-[2px]" style={{ background: MODEL_COLOR }} />
          按任务开始日归属的花费
        </span>
        <span className="text-ink-3">柱顶「+」：那天有任务没价格</span>
        <span className="text-ink-3">服务端快照没有逐日请求，柱子是任务花费的日归属。</span>
      </div>
    </div>
  );
}

function rankMax(rows: MonitorUsageRow[]): number {
  return Math.max(1, ...rows.map((row) => row.requests));
}

function TaskCard({ task }: { task: MonitorTaskSummary }) {
  const usedPercent = task.contextWindow ? Math.round((task.contextUsed / task.contextWindow) * 100) : null;
  return (
    <button
      type="button"
      data-kind="task-card"
      onClick={() => go('monitor:task:' + task.taskId)}
      className="group flex min-w-0 flex-col rounded-card border border-line bg-raised/40 p-3 text-left transition-colors hover:border-line-2 hover:bg-hover"
    >
      <span className="flex items-center gap-1.5 text-1 text-ink-3">
        <span className={cn(task.running ? 'text-run' : 'text-ink-3')}>{stateLabel(task.state)}</span>
        <span className="rounded-badge bg-hover px-1.5 py-[1px]">未归项目</span>
        <span className="ml-auto brand-type">{task.attemptId ? '第 ' + task.attemptId + ' 次' : ''}</span>
      </span>
      <span className="mt-1 block truncate text-2 font-medium text-ink">{task.title}</span>
      <span className="mt-2 flex items-baseline gap-2">
        <span className="brand-type text-4 text-ink">
          {task.costUsd == null ? <Missing /> : formatUsd(task.costUsd, task.unpriced)}
        </span>
        <span className="text-1 text-ink-3">{formatTokens(task.inputTokens + task.outputTokens)} token</span>
        <span className="ml-auto text-1 text-ink-3 group-hover:text-ink-2">
          诊断 <ArrowUpRight className="inline size-3" />
        </span>
      </span>
      <span className="mt-2 text-1 text-ink-3">
        {usedPercent == null ? <Missing what="上下文没采到" /> : '上下文 ' + usedPercent + '% · ' + formatTokens(task.contextUsed) + ' / ' + formatTokens(task.contextWindow)}
      </span>
    </button>
  );
}

export function CostTab() {
  const snapshot = useMonitorStore((state) => state.snapshot);
  const error = useMonitorStore((state) => state.snapshotError);
  const loading = useMonitorStore((state) => state.loading);
  const range = useMonitorStore((state) => state.range);
  const setRange = useMonitorStore((state) => state.setRange);
  const tasks = snapshot?.tasks ?? [];
  const models = snapshot?.models ?? [];
  const tools = snapshot?.tools ?? [];
  const commands = snapshot?.commands ?? [];
  const rings = snapshot?.rings;

  if (error) {
    return (
      <Card title="监控读不出来">
        <p className="text-2 text-fail">{error}</p>
        <p className="mt-1 text-1 text-ink-3">快照只读服务端事实；没有数据时不让页面猜。</p>
      </Card>
    );
  }
  if (!snapshot) {
    return <div className="grid h-40 place-items-center text-2 text-ink-3" data-kind="loading">{loading ? '正在取监控快照…' : '没采到监控快照'}</div>;
  }

  const moneyParts = models
    .filter((row) => row.costUsd != null)
    .map((row) => ({ key: row.key, name: row.label, value: row.costUsd ?? 0, color: MODEL_COLOR }));
  const timeParts = [
    { key: 'request', name: '模型请求', value: rings?.requestMs ?? 0, color: MODEL_COLOR },
    { key: 'tool', name: '工具', value: rings?.toolMs ?? 0, color: TOOL_COLOR },
  ];
  const sortedTasks = [...tasks].sort((left, right) => (right.costUsd ?? 0) - (left.costUsd ?? 0));
  const totalRequests = tasks.reduce((sum, task) => sum + task.requests, 0);
  const totalTools = tools.reduce((sum, row) => sum + row.requests, 0);

  return (
    <div data-tab="cost" className="space-y-4 pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          value={range}
          onChange={setRange}
          options={[
            ['today', '今天'],
            ['last7-days', '近 7 天'],
            ['month', '本月'],
          ]}
        />
        <span className="text-1 text-ink-3">
          用户时区 {snapshot.timezone}；服务端 {new Date(snapshot.generatedAt).toLocaleTimeString('zh-CN', { hour12: false })} 现算
        </span>
        {loading ? <span className="text-1 text-ink-3">刷新中…</span> : null}
      </div>

      <div className="grid grid-cols-2 gap-3 md:grid-cols-5" data-kind="kpis">
        <Kpi
          label="花费"
          value={rings?.costUsd == null ? <Missing /> : formatUsd(rings.costUsd, rings.unpriced)}
          sub="含 Kind / 后台；无价轮不计"
        />
        <Kpi label="Token" value={rings ? formatTokens(rings.tokens) : <Missing />} sub="输入 + 输出" />
        <Kpi label="请求" value={totalRequests ? totalRequests : <Missing what="没采到" />} sub="模型请求次数" />
        <Kpi label="工具" value={totalTools ? totalTools : <Missing what="没采到" />} sub="工具调用次数" />
        <Kpi label="命令" value={commands.reduce((sum, row) => sum + row.requests, 0) || <Missing what="没跑过" />} sub="bash 程序次数" />
      </div>

      <Card title="每天的柱" note="花费按任务开始日归属·范围外素灰">
        <DailyBars range={range} tasks={tasks} />
      </Card>

      <div className="grid gap-3 md:grid-cols-2">
        <Donut
          title="钱花在哪"
          parts={moneyParts}
          center={formatUsd(rings?.costUsd, rings?.unpriced)}
          missing={<Missing what="没有带价格的花费" />}
        />
        <Donut
          title="时间花在哪"
          parts={timeParts}
          center={rings ? fmtClock((rings.requestMs + rings.toolMs) / 1000) : undefined}
          missing={<Missing what="时长没采到" />}
        />
      </div>

      <div className="grid gap-3 lg:grid-cols-3">
        <Card title="模型" note="请求数 · 颜色">
          {models.length ? (
            models.map((row) => (
              <RankRow key={row.key} name={row.label} count={row.requests} max={rankMax(models)} color={MODEL_COLOR} mono right={formatUsd(row.costUsd, row.unpriced)} />
            ))
          ) : (
            <p className="text-2 text-ink-3"><Missing what="没采到模型请求" /></p>
          )}
        </Card>
        <Card title="工具" note="调用次数">
          {tools.length ? (
            tools.map((row) => (
              <RankRow key={row.key} name={row.label} count={row.requests} max={rankMax(tools)} color={TOOL_COLOR} mono />
            ))
          ) : (
            <p className="text-2 text-ink-3"><Missing what="没采到工具调用" /></p>
          )}
        </Card>
        <Card title="命令" note="bash 里跑的程序">
          {commands.length ? (
            commands.map((row) => (
              <RankRow key={row.key} name={row.label} count={row.requests} max={rankMax(commands)} color={CMD_COLOR} mono />
            ))
          ) : (
            <p className="text-2 text-ink-3"><Missing what="没跑过命令" /></p>
          )}
        </Card>
      </div>

      <div>
        <div className="mb-3 mt-6 flex flex-wrap items-center gap-2">
          <h2 className="text-3 font-semibold text-ink">任务</h2>
          <span className="text-1 text-ink-3">{tasks.length} 件 · 点开看诊断</span>
        </div>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3" data-kind="task-cards">
          {sortedTasks.map((task) => (
            <TaskCard key={task.taskId} task={task} />
          ))}
          <div className="flex min-w-0 flex-col rounded-card border border-dashed border-line p-3" data-kind="kind-card">
            <div className="flex items-center gap-2 text-2 text-ink">
              <Sparkles className="size-[18px] text-ink-3" />
              Kind 对话
            </div>
            <div className="mt-2 text-2 text-ink-2">已并入上面的合计和环</div>
            <div className="text-1 text-ink-3">不分项目，不做诊断</div>
          </div>
        </div>
        {tasks.length === 0 ? (
          <p className="mt-3 text-2 text-ink-3"><Missing what="还没有任务" /></p>
        ) : null}
      </div>

      <div className="flex flex-wrap gap-x-4 gap-y-1 text-1 text-ink-2">
        <span className="flex items-center gap-1.5">
          <i className="size-2 rounded-[2px]" style={{ background: MODEL_COLOR }} />模型
        </span>
        <span className="flex items-center gap-1.5">
          <i className="size-2 rounded-[2px]" style={{ background: TOOL_COLOR }} />工具
        </span>
        <span className="flex items-center gap-1.5">
          <i className="size-2 rounded-[2px]" style={{ background: CMD_COLOR }} />命令
        </span>
        <span className="flex items-center gap-1.5">
          <i className="size-2 rounded-[2px]" style={{ background: KIND_COLOR }} />Kind
        </span>
      </div>
    </div>
  );
}
