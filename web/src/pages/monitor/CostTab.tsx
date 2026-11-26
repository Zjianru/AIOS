/**
 * 监控 · 消耗：范围、五张大数字、每天柱按项目堆、钱/时间两个环、模型/工具/命令排行、任务卡。
 * 照原型 v1.1：五卡（花费/Token/缓存命中/模型请求/在忙）、项目筛选一排标签、
 * 日柱按项目叠色（今天也显示整月柱、只高亮今天）、时间环分段、排行按项目拆并跟筛选。
 * Kind 只进合计，不产任务卡、不做诊断。
 */

import { useEffect, useMemo, useState } from 'react';
import { ArrowUpRight, Sparkles } from 'lucide-react';
import { go } from '@/app/navigate';
import { formatCacheHitPercent, formatTokens } from '@/lib/numbers';
import { cn } from '@/lib/utils';
import { useRouteStore } from '@/app/routeStore';
import { useMonitorStore } from './monitorStore';
import { matchesProject, useProjectName, useProjectStore } from '@/pages/project/projectStore';
import { monitorSnapshot } from '@/data/service';
import type { MonitorDay, MonitorTaskSummary, MonitorUsageRow } from '@/data/service';
import type { MonitorDayGroup } from '@/data/generated/MonitorDayGroup';
import { Card, Donut, Kpi, Missing, RankRow, Segmented, fmtClock } from './monitorParts';

const MODEL_COLOR = 'var(--kf-c1)';
const TOOL_COLOR = 'var(--kf-c4)';
const CMD_COLOR = 'var(--kf-c3)';
const KIND_COLOR = 'var(--kf-c5)';
const PROJECT_COLORS = ['var(--kf-c2)', 'var(--kf-c6)', 'var(--kf-c7)', 'var(--kf-c8)', 'var(--kf-c4)'];
const PERSONAL_COLOR = 'var(--ink-3)';

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

function timezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

function groupColor(projectId: string | null | undefined, projects: { id: string; name: string }[]): string {
  if (!projectId) return PERSONAL_COLOR;
  const index = projects.findIndex((project) => project.id === projectId);
  return index >= 0 ? PROJECT_COLORS[index % PROJECT_COLORS.length] : MODEL_COLOR;
}

function groupName(group: MonitorDayGroup, projects: { id: string; name: string }[]): string {
  if (group.projectId) return projects.find((project) => project.id === group.projectId)?.name ?? group.projectId;
  return group.label || '个人';
}

function useMonthDays(range: 'today' | 'last7-days' | 'month'): MonitorDay[] {
  const [days, setDays] = useState<MonitorDay[] | null>(null);
  useEffect(() => {
    if (range !== 'today') {
      setDays(null);
      return;
    }
    let alive = true;
    void monitorSnapshot('month', timezone())
      .then((snapshot) => {
        if (alive) setDays(snapshot.days);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [range]);
  return days ?? [];
}

function DailyBars({
  days,
  filter,
  range,
  projects,
}: {
  days: MonitorDay[];
  filter: string | null;
  range: 'today' | 'last7-days' | 'month';
  projects: { id: string; name: string }[];
}) {
  if (!days.length) {
    return (
      <div data-kind="cost-daily" className="grid h-[168px] place-items-center text-2 text-ink-3">
        <Missing what="这段时间没有落天事件" />
      </div>
    );
  }
  const inFilter = (group: MonitorDayGroup) => !filter || matchesProject(group.projectId, filter);
  const series = filter
    ? [
        {
          key: 'filter',
          name: filter === 'personal' ? '个人' : '这个项目',
          color: groupColor(filter === 'personal' ? null : filter, projects),
          of: (day: MonitorDay) => day.groups.filter(inFilter).reduce((sum, group) => sum + (group.costUsd ?? 0), 0),
        },
      ]
    : [
        ...projects.map((project) => ({
          key: project.id,
          name: project.name,
          color: groupColor(project.id, projects),
          of: (day: MonitorDay) =>
            day.groups.filter((group) => group.projectId === project.id).reduce((sum, group) => sum + (group.costUsd ?? 0), 0),
        })),
        {
          key: 'personal',
          name: '个人',
          color: PERSONAL_COLOR,
          of: (day: MonitorDay) =>
            day.groups
              .filter((group) => !group.projectId && group.label !== 'Kind 对话')
              .reduce((sum, group) => sum + (group.costUsd ?? 0), 0),
        },
        {
          key: 'kind',
          name: 'Kind 对话',
          color: KIND_COLOR,
          of: (day: MonitorDay) =>
            day.groups.filter((group) => group.label === 'Kind 对话').reduce((sum, group) => sum + (group.costUsd ?? 0), 0),
        },
      ];
  const totalOf = (day: MonitorDay) => day.groups.filter(inFilter).reduce((sum, group) => sum + (group.costUsd ?? 0), 0);
  const max = Math.max(...days.map(totalOf), 0.01);
  const top = Math.ceil(max * 2) / 2;
  const today = new Date().toLocaleDateString('sv-SE');
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
            {days.map((day) => {
              const stack = series
                .map((item) => ({ ...item, value: item.of(day) }))
                .filter((item) => item.value > 0);
              const total = totalOf(day);
              const dim = !filter && range === 'today' && day.day !== today;
              return (
                <div
                  key={day.day}
                  data-kind="cost-day"
                  data-day={day.day}
                  title={day.day + ' · 请求 ' + day.requests + ' · 工具 ' + day.toolCalls + ' · 命令 ' + day.commandCalls}
                  className="relative flex h-full min-w-0 flex-1 flex-col justify-end"
                >
                  <div className="flex flex-col-reverse gap-[2px]" style={{ height: Math.max(total ? 2 : 0, (total / top) * 100) + '%' }}>
                    {stack.map((item, index) => (
                      <i
                        key={item.key + index}
                        data-kind="cost-bar-group"
                        data-project-id={item.key}
                        title={item.name + ' ' + formatUsd(item.value)}
                        className={cn('block min-h-[2px] w-full', index === stack.length - 1 && 'rounded-t-[4px]')}
                        style={{ flexGrow: item.value, flexBasis: 0, background: dim ? 'var(--line-2)' : item.color }}
                      />
                    ))}
                  </div>
                  {day.unpriced ? (
                    <span className="brand-type absolute bottom-full left-1/2 mb-0.5 -translate-x-1/2 text-1 leading-none text-ink-3">+</span>
                  ) : null}
                </div>
              );
            })}
          </div>
          <div className="brand-type flex h-5 items-end gap-[4px] text-1 text-ink-3">
            {days.map((day) => (
              <span key={day.day} className={cn('min-w-0 flex-1 text-center', day.day === today && 'text-ink')}>
                {day.day.slice(5)}
              </span>
            ))}
          </div>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 pl-14 text-1 text-ink-2">
        {series.map((item) => (
          <span key={item.key} className="flex items-center gap-1.5">
            <i className="size-2 rounded-[2px]" style={{ background: item.color }} />
            {item.name}
          </span>
        ))}
        <span className="text-ink-3">今天也画整月柱，只高亮今天</span>
        <span className="text-ink-3">柱顶「+」：那天有轮没价格</span>
      </div>
    </div>
  );
}

function TaskCard({ task }: { task: MonitorTaskSummary }) {
  const projectName = useProjectName(task.projectId);
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
        <span data-kind="task-project" className="rounded-badge bg-hover px-1.5 py-[1px]">
          {projectName}
        </span>
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

function sumRows(rows: MonitorUsageRow[]) {
  return {
    input: rows.reduce((sum, row) => sum + row.inputTokens, 0),
    output: rows.reduce((sum, row) => sum + row.outputTokens, 0),
    cacheRead: rows.reduce((sum, row) => sum + row.cacheReadTokens, 0),
    requests: rows.reduce((sum, row) => sum + row.requests, 0),
  };
}

export function CostTab() {
  const snapshot = useMonitorStore((state) => state.snapshot);
  const route = useRouteStore((state) => state.route);
  const projectFilter = route.query.get('p');
  const projects = useProjectStore((state) => state.projects);
  const error = useMonitorStore((state) => state.snapshotError);
  const loading = useMonitorStore((state) => state.loading);
  const range = useMonitorStore((state) => state.range);
  const setRange = useMonitorStore((state) => state.setRange);
  const monthDays = useMonthDays(range);
  const projectsList = projects ?? [];
  const tasks = (snapshot?.tasks ?? []).filter((task) => matchesProject(task.projectId, projectFilter));
  const models = (snapshot?.models ?? []).filter((row) => matchesProject(row.projectId, projectFilter));
  const tools = (snapshot?.tools ?? []).filter((row) => matchesProject(row.projectId, projectFilter));
  const commands = (snapshot?.commands ?? []).filter((row) => matchesProject(row.projectId, projectFilter));
  const rings = snapshot?.rings;

  const dayGroups = useMemo(() => {
    const groups = new Map<string, { key: string; name: string; color: string; value: number }>();
    for (const day of snapshot?.days ?? []) {
      for (const group of day.groups) {
        if (projectFilter && !matchesProject(group.projectId, projectFilter)) continue;
        const key = group.projectId ?? (group.label === 'Kind 对话' ? 'kind' : 'personal');
        const current = groups.get(key) ?? {
          key,
          name: groupName(group, projectsList),
          color: group.label === 'Kind 对话' ? KIND_COLOR : groupColor(group.projectId, projectsList),
          value: 0,
        };
        current.value += group.costUsd ?? 0;
        groups.set(key, current);
      }
    }
    return [...groups.values()].filter((group) => group.value > 0);
  }, [projectFilter, projectsList, snapshot?.days]);

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

  const rowStats = sumRows(models);
  const priced = tasks.map((task) => task.costUsd).filter((value): value is number => value != null);
  const projectCost = projectFilter
    ? (priced.length ? priced.reduce((sum, value) => sum + value, 0) : null)
    : (rings?.costUsd ?? null);
  const partial = projectFilter ? tasks.some((task) => task.costUsd == null || task.unpriced) : (rings?.unpriced ?? false);
  const tokens = projectFilter ? rowStats.input + rowStats.output : (rings?.tokens ?? 0);
  const cacheHit = formatCacheHitPercent(rowStats.cacheRead, rowStats.input);
  const busyMs = projectFilter ? null : (rings?.busyMs ?? (rings ? rings.requestMs + rings.toolMs + rings.commandMs : null));
  // 五段照原型：等首字/没采到内核没有这个数，画 0 并在名字里照实写；不补假数。
  const timeParts = projectFilter
    ? []
    : [
        { key: 'ttft', name: '等首字（没采到）', value: 0, color: 'var(--line-2)' },
        { key: 'gen', name: '模型生成', value: rings?.requestMs ?? 0, color: MODEL_COLOR },
        { key: 'tool', name: '工具', value: rings?.toolMs ?? 0, color: TOOL_COLOR },
        { key: 'cmd', name: '命令', value: rings?.commandMs ?? 0, color: CMD_COLOR },
        { key: 'unknown', name: '没采到', value: 0, color: 'var(--line-3)' },
      ];
  const sortedTasks = [...tasks].sort((left, right) => (right.costUsd ?? 0) - (left.costUsd ?? 0));
  const daysForBars = range === 'today' ? (monthDays.length ? monthDays : snapshot.days) : snapshot.days;
  const rankProject = (row: MonitorUsageRow) =>
    row.projectId ? (projectsList.find((project) => project.id === row.projectId)?.name ?? row.projectId) : '个人';

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
        {projectFilter ? (
          <span className="text-1 text-ink-3" data-kind="project-scope-note">
            日柱/时间环/全站合计无法按项目拆开；排行只合计这个项目，不混全量。
          </span>
        ) : null}
      </div>

      {projectFilter && tasks.length === 0 ? (
        <div data-kind="project-empty" className="rounded-card border border-dashed border-line px-4 py-8 text-center text-2 text-ink-3">
          这个项目还没有任务。全站合计和日柱不混进来；去 Kind 派一个归到这个项目的任务再看。
        </div>
      ) : null}
      <div className="grid grid-cols-2 gap-3 md:grid-cols-5" data-kind="kpis">
        <Kpi label="花费" value={projectCost == null ? <Missing /> : formatUsd(projectCost, partial)} sub={projectFilter ? '只算这个项目' : '含 Kind / 后台；无价轮不计'} />
        <Kpi label="Token" value={formatTokens(tokens)} sub={'输入 ' + formatTokens(rowStats.input) + ' · 输出 ' + formatTokens(rowStats.output)} />
        <Kpi label="缓存命中" value={cacheHit ?? <Missing what="没有输入" />} sub={'读缓存 ' + formatTokens(rowStats.cacheRead)} />
        <Kpi label="模型请求" value={rowStats.requests.toLocaleString('en-US')} sub={'工具调用 ' + tools.reduce((sum, row) => sum + row.requests, 0).toLocaleString('en-US') + ' 次'} />
        <Kpi
          label="在忙"
          value={busyMs == null ? <Missing what="无法按项目拆" /> : fmtClock(busyMs / 1000)}
          sub={'模型 ' + fmtClock((rings?.requestMs ?? 0) / 1000) + ' · 工具 ' + fmtClock((rings?.toolMs ?? 0) / 1000)}
        />
      </div>

      <Card title="每天的花费" note="按项目堆；今天也显示整月柱，只高亮今天">
        <DailyBars days={daysForBars} filter={projectFilter} range={range} projects={projectsList} />
      </Card>

      <div className="grid gap-3 lg:grid-cols-2">
        <Donut
          title="钱花在哪"
          parts={dayGroups}
          center={projectCost == null ? '没采到' : formatUsd(projectCost, partial)}
          missing={<Missing what="没有带价格的花费" />}
        />
        <div data-kind="time-donut">
          <Donut
            title="时间花在哪"
            parts={timeParts}
            center={busyMs == null ? <Missing what="无法按项目拆" /> : fmtClock(busyMs / 1000)}
            missing={projectFilter ? <Missing what="无法按项目拆开" /> : <Missing what="时长没采到" />}
          />
          <p className="mt-1 text-1 text-ink-3" data-kind="time-ring-missing">
            等首字 / 没采到：内核没给分段，照实不画。
          </p>
        </div>
      </div>

      <div className="grid gap-3 lg:grid-cols-3">
        <Card title="模型" note="请求次数 · 花费（按项目）">
          {models.length ? (
            models.map((row) => (
              <RankRow
                key={(row.projectId ?? 'personal') + ':' + row.key}
                name={(projectFilter ? '' : rankProject(row) + ' · ') + row.label}
                count={row.requests}
                max={Math.max(1, ...models.map((item) => item.requests))}
                color={MODEL_COLOR}
                mono
                right={formatUsd(row.costUsd, row.unpriced)}
              />
            ))
          ) : (
            <p className="text-2 text-ink-3"><Missing what="没采到模型请求" /></p>
          )}
        </Card>
        <Card title="工具" note="调用次数（按项目）">
          {tools.length ? (
            tools.map((row) => (
              <RankRow
                key={(row.projectId ?? 'personal') + ':' + row.key}
                name={(projectFilter ? '' : rankProject(row) + ' · ') + row.label}
                count={row.requests}
                max={Math.max(1, ...tools.map((item) => item.requests))}
                color={TOOL_COLOR}
                mono
              />
            ))
          ) : (
            <p className="text-2 text-ink-3"><Missing what="没采到工具调用" /></p>
          )}
        </Card>
        <Card title="命令" note="bash 里跑的程序（按项目）">
          {commands.length ? (
            commands.map((row) => (
              <RankRow
                key={(row.projectId ?? 'personal') + ':' + row.key}
                name={(projectFilter ? '' : rankProject(row) + ' · ') + row.label}
                count={row.requests}
                max={Math.max(1, ...commands.map((item) => item.requests))}
                color={CMD_COLOR}
                mono
              />
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
          {!projectFilter ? (
            <div className="flex min-w-0 flex-col rounded-card border border-dashed border-line p-3" data-kind="kind-card">
              <div className="flex items-center gap-2 text-2 text-ink">
                <Sparkles className="size-[18px] text-ink-3" />
                Kind 对话
              </div>
              <div className="brand-type mt-2 text-5 text-ink">{formatUsd(rings?.costUsd ?? null, rings?.unpriced)}</div>
              <div className="text-1 text-ink-3">不分项目，不做诊断</div>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
