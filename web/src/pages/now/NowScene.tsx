/**
 * 首页「现在」（早先票；本票 照原型做全）。
 *
 * 三组：等你决定（工作页同一份 Work.wait）/ 出了问题 / 在推进（按总任务一张卡：站点链、卡在哪句、
 * 模型·已用·token·花费；「做到几成」「还需」内核没有，空着不写——reply s）。
 * 摘要显示成纯文字；顶上是通知说明，页尾两句说明。数据只用现有事实：Work 快照 + graphModel + 监控快照。
 */

import { useEffect, useMemo, useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { go } from '@/app/navigate';
import { useProjectStore } from '@/pages/project/projectStore';
import { deriveGraphs } from '@/pages/work/graphModel';
import type { TaskGraph } from '@/pages/work/graphModel';
import { WaitRow } from '@/pages/work/WaitList';
import { advancingOf, boardOf, problemsOf, stageLabel, weekDone } from '@/pages/work/workModel';
import { useWorkPolling, useWorkStore } from '@/pages/work/workStore';
import { useSettingsState } from '@/lib/settingsState';
import { clockOf, formatCost } from '@/lib/numbers';
import { monitorSnapshot } from '@/data/service';
import type { MonitorTaskSummary } from '@/data/generated/MonitorTaskSummary';
import type { TaskFact } from '@/data/generated/TaskFact';
import { plainText } from '@/lib/plain';
import { cn } from '@/lib/utils';

const MAX_PER_GROUP = 3;

function greetingOf(hour: number): string {
  if (hour < 6) return '夜深了';
  if (hour < 11) return '早上好';
  if (hour < 13) return '中午好';
  if (hour < 18) return '下午好';
  return '晚上好';
}

function fmtDur(ms: number): string {
  const minutes = Math.max(0, Math.round(ms / 60_000));
  if (minutes < 60) return minutes + ' 分钟';
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? hours + ' 小时 ' + rest + ' 分' : hours + ' 小时';
}

function fmtTokens(tokens: number): string {
  if (tokens >= 1_000_000) return (tokens / 1_000_000).toFixed(1) + 'M';
  if (tokens >= 1000) return (tokens / 1000).toFixed(1) + 'k';
  return String(tokens);
}

/** 问题卡头行时间：任务最近一份报告的时间，没有就用建任务时间。 */
function problemTime(task: TaskFact): string {
  const at = task.latest?.at ?? task.createdAt;
  return at ? clockOf(at / 1000) : '';
}

function GroupHeader({ tone, label, count, hint }: { tone: string; label: string; count: number; hint?: string }) {
  return (
    <div className="mb-2 flex flex-wrap items-center gap-2">
      <span className="size-[9px] rounded-full" style={{ backgroundColor: tone }} />
      <h2 className="text-3 font-medium text-ink">{label}</h2>
      <span className="brand-type text-1 text-ink-3">{count}</span>
      {hint ? <span className="ml-auto text-1 text-ink-3">{hint}</span> : null}
    </div>
  );
}

function MoreButton({ hidden, more, onClick }: { hidden: boolean; more: number; onClick: () => void }) {
  if (hidden || more <= 0) return null;
  return (
    <button
      type="button"
      data-kind="now-more"
      onClick={onClick}
      className="mt-1.5 text-1 text-ink-3 transition-colors hover:text-ink-2"
    >
      还有 {more} 条 <span aria-hidden="true">›</span>
    </button>
  );
}

function ProjectLine({ projectId }: { projectId: string | null | undefined }) {
  const projects = useProjectStore((state) => state.projects);
  const name = projects?.find((project) => project.id === projectId)?.name ?? '个人';
  return <>{name}</>;
}

interface AdvancingCard {
  key: string;
  title: string;
  projectId: string | null | undefined;
  taskId: string;
  chain: { id: string; title: string; state: string; running: boolean }[];
  stuck: string;
  /** 监控快照里这个任务有没有事实行；没有就照实写「没采到」，不 reduce 出 0。 */
  hasData: boolean;
  model: string | null;
  usedMs: number | null;
  tokens: number | null;
  cost: number | null;
  /** 有事实行但没价格（unpriced）→ 显示「没价格」。 */
  unpriced: boolean;
  /** 这张卡里等你处理的件数；>0 时头行出「等你 · N」胶囊。 */
  waiting: number;
  /** 卡里有没有出了问题的站（等待处理且没等你）。 */
  troubled: boolean;
}

function buildAdvancing(
  work: NonNullable<ReturnType<typeof useWorkStore.getState>['work']>,
  advancing: TaskFact[],
  monitor: Record<string, MonitorTaskSummary>,
  now: number,
): AdvancingCard[] {
  const graphs = deriveGraphs(work);
  const taskToGraph = new Map<string, TaskGraph>();
  for (const graph of graphs.graphs) for (const node of graph.nodes) taskToGraph.set(node.task.id, graph);
  const waitOf = new Map<string, string>();
  const waitCount = new Map<string, number>();
  for (const item of work.wait) {
    if (item.taskId) {
      waitCount.set(item.taskId, (waitCount.get(item.taskId) ?? 0) + 1);
      if (!waitOf.has(item.taskId)) waitOf.set(item.taskId, plainText(item.need || item.body));
    }
  }
  const facts = (ids: string[]) => {
    const rows = ids.map((id) => monitor[id]).filter((row): row is MonitorTaskSummary => Boolean(row));
    const hasData = rows.length > 0;
    const prices = rows.map((row) => row.costUsd).filter((value): value is number => value != null);
    const started = rows.map((row) => row.startedAt).filter((value): value is number => value != null);
    const ended = rows.map((row) => row.endedAt).filter((value): value is number => value != null);
    const running = rows.some((row) => row.running);
    // 没有事实行就不给 0：token/已用/花费写「没采到」；有行但没价格写「没价格」。
    const usedMs = !hasData
      ? null
      : running && started.length > 0
        ? now - Math.min(...started)
        : started.length > 0 && ended.length > 0
          ? Math.max(...ended) - Math.min(...started)
          : null;
    return {
      hasData,
      model: rows.find((row) => row.model)?.model ?? null,
      tokens: hasData ? rows.reduce((sum, row) => sum + row.inputTokens + row.outputTokens, 0) : null,
      cost: hasData && prices.length > 0 ? prices.reduce((sum, value) => sum + value, 0) : null,
      unpriced: hasData && prices.length === 0,
      usedMs,
    };
  };
  const cards: AdvancingCard[] = [];
  const seen = new Set<string>();
  for (const task of advancing) {
    const graph = taskToGraph.get(task.id);
    if (graph) {
      if (seen.has(graph.id)) continue;
      seen.add(graph.id);
      const members = [...graph.nodes].sort((left, right) => left.depth - right.depth).map((node) => node.task);
      const waiting = members.reduce((sum, member) => sum + (waitCount.get(member.id) ?? 0), 0);
      const troubled = members.some((member) => member.stage === 'needs_conditions');
      const stuck =
        members.map((member) => waitOf.get(member.id)).find(Boolean) ??
        members
          .map((member) =>
            member.latest?.question
              ? plainText(member.latest.question)
              : member.latest?.summary
                ? plainText(member.latest.summary)
                : '',
          )
          .find(Boolean) ??
        '在跑，还没交结果。';
      cards.push({
        key: graph.id,
        title: graph.title,
        projectId: members[0]?.projectId,
        taskId: graph.id,
        chain: members.map((member) => ({
          id: member.id,
          title: member.title,
          state: stageLabel(member.stage),
          running: member.running,
        })),
        stuck,
        ...facts(members.map((member) => member.id)),
        waiting,
        troubled,
      });
    } else {
      const stuck =
        waitOf.get(task.id) ??
        (task.latest?.question
          ? plainText(task.latest.question)
          : task.latest?.summary
            ? plainText(task.latest.summary)
            : '在跑，还没交结果。');
      cards.push({
        key: task.id,
        title: task.title,
        projectId: task.projectId,
        taskId: task.id,
        chain: [{ id: task.id, title: task.title, state: stageLabel(task.stage), running: task.running }],
        stuck,
        ...facts([task.id]),
        waiting: waitCount.get(task.id) ?? 0,
        troubled: task.stage === 'needs_conditions',
      });
    }
  }
  return cards;
}

function AdvancingRow({ card }: { card: AdvancingCard }) {
  const money = useSettingsState((state) => state.money);
  return (
    <article data-kind="now-advancing-card" className="rounded-card border border-line bg-pane/40 p-2.5">
      <div className="flex flex-wrap items-baseline gap-1.5">
        <h3 className="min-w-0 flex-1 truncate text-2 font-medium text-ink">{card.title}</h3>
        <span className="shrink-0 text-1 text-ink-3">
          <ProjectLine projectId={card.projectId} />
        </span>
        {card.waiting > 0 ? (
          <span data-kind="now-carrying-wait" className="shrink-0 rounded-full bg-wait-soft px-2 py-[1px] text-1 text-wait">
            等你 · {card.waiting}
          </span>
        ) : card.troubled ? (
          <span data-kind="now-carrying-trouble" className="shrink-0 rounded-full bg-fail-soft px-2 py-[1px] text-1 text-fail">
            有问题 · Kind 处理中
          </span>
        ) : null}
      </div>
      <div className="mt-1.5 flex flex-wrap items-stretch gap-1.5">
        {card.chain.map((station, index) => (
          <span key={station.id} className="flex items-center gap-1.5">
            {index > 0 ? <ChevronRight className="size-3 shrink-0 text-ink-3" aria-hidden="true" /> : null}
            <button
              type="button"
              data-kind="now-station"
              onClick={() => go('taskchat:' + station.id)}
              className="rounded-btn border border-line px-1.5 py-1 text-left transition-colors hover:bg-hover"
            >
              <span className="flex items-center gap-1 text-1 text-ink-3">
                <span
                  className="size-[6px] rounded-full"
                  style={{ backgroundColor: station.running ? 'var(--run)' : 'var(--line-2)' }}
                />
                {station.state}
              </span>
              <span className="mt-0.5 block max-w-[150px] truncate text-1 text-ink-2">{station.title}</span>
              <span className="block text-1 text-ink-3">ThinAgent</span>
            </button>
          </span>
        ))}
      </div>
      <p className="mt-1.5 line-clamp-2 text-1 leading-4 text-ink-2" data-kind="now-stuck">
        卡在：{card.stuck}
      </p>
      <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-1 text-ink-3" data-kind="now-advancing-meta">
        <span>{card.model ?? (card.hasData ? '模型未知' : '没采到')}</span>
        <span>·</span>
        <span>已用 {card.usedMs != null && card.usedMs > 0 ? fmtDur(card.usedMs) : '没采到'}</span>
        <span>·</span>
        <span>还需 —</span>
        <span>·</span>
        <span>{card.tokens != null ? fmtTokens(card.tokens) + ' token' : 'token 没采到'}</span>
        <span>·</span>
        <span>
          {card.cost != null
            ? formatCost(card.cost, money.currency, money.cnyPerUsd, card.unpriced)
            : card.unpriced
              ? '没价格'
              : '花费没采到'}
        </span>
        <button
          type="button"
          data-kind="now-advancing-open"
          onClick={() => go('taskchat:' + card.taskId)}
          className="ml-auto rounded-btn px-1.5 py-0.5 transition-colors hover:bg-hover hover:text-ink-2"
        >
          展开 · 去看 <span aria-hidden="true">›</span>
        </button>
      </div>
    </article>
  );
}

function ProblemRow({ task }: { task: TaskFact }) {
  const work = useWorkStore((state) => state.work);
  const projectName = useProjectStore((state) => state.projects?.find((project) => project.id === task.projectId)?.name ?? '个人');
  const inDecisions = work?.wait.some((item) => item.taskId === task.id) ?? false;
  // 类别（LLM/AIOS）内核没有事实：不冒充；这里只陈述发生了什么 + Kind 怎么处理。
  const what = task.latest?.question
    ? plainText(task.latest.question)
    : task.latest?.summary
      ? plainText(task.latest.summary)
      : 'Kind 处理不了，停下来等你。';
  const handling = task.latest?.question
    ? 'Kind 已经问过你，还没得到答复，先停在这里。'
    : task.latest?.summary
      ? 'Kind 的处理：' + plainText(task.latest.summary)
      : 'Kind 会先自己处理；处理不了才变成上面的决定。';
  return (
    <button
      type="button"
      data-kind="now-problem"
      onClick={() => go('taskchat:' + task.id)}
      className="block w-full rounded-card border border-line bg-pane/40 p-2.5 text-left transition-colors hover:bg-hover"
    >
      <span className="flex flex-wrap items-center gap-1.5 text-1 text-ink-3">
        <span className="text-fail">出了问题</span>
        <span data-kind="now-problem-origin" className="min-w-0 truncate">
          {projectName} › {task.title}
        </span>
        <span className="rounded-badge bg-hover px-1.5 py-[1px]">
          {inDecisions ? '已转为决定' : '处理中'}
        </span>
        <span className="ml-auto brand-type" data-kind="now-problem-time">
          {problemTime(task)}
        </span>
      </span>
      <span className="mt-1 block text-2 text-ink" data-kind="now-problem-what">
        {what}
      </span>
      <span className="mt-0.5 block truncate text-1 text-ink-2" data-kind="now-problem-handling">
        {handling}
      </span>
    </button>
  );
}

export function NowScene() {
  useWorkPolling();
  const work = useWorkStore((state) => state.work);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [notifyOpen, setNotifyOpen] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [monitor, setMonitor] = useState<Record<string, MonitorTaskSummary>>({});

  useEffect(() => {
    setNow(Date.now());
  }, [work]);

  // 花费按设置里的币种显示（CN 用户是 ¥）；取不到就沿用上次值，不写死 $。
  useEffect(() => {
    void useSettingsState.getState().refreshMoney();
  }, []);

  useEffect(() => {
    let alive = true;
    const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Shanghai';
    void monitorSnapshot('today', timezone)
      .then((snapshot) => {
        if (alive) setMonitor(Object.fromEntries(snapshot.tasks.map((row) => [row.taskId, row])));
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const board = useMemo(() => boardOf(work?.tasks ?? [], work?.wait ?? []), [work]);
  const problems = useMemo(() => problemsOf(work?.tasks ?? [], work?.wait ?? []), [work]);
  const advancing = useMemo(() => advancingOf(board), [board]);
  const doneThisWeek = useMemo(() => weekDone(work?.tasks ?? [], now), [work, now]);
  const cards = useMemo(() => (work ? buildAdvancing(work, advancing, monitor, now) : []), [work, advancing, monitor, now]);

  if (!work) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取「现在」…</div>;
  }

  const allEmpty = work.wait.length === 0 && problems.length === 0 && advancing.length === 0;
  const greeting = greetingOf(new Date().getHours());
  const shown = (key: string, total: number): number => (expanded[key] ? total : Math.min(total, MAX_PER_GROUP));

  return (
    <div className="h-full overflow-y-auto" data-scene-page="now">
      <div className="mx-auto w-full max-w-[860px] px-6 py-6">
        <h1 className="text-6 font-semibold leading-8 text-ink">{greeting}</h1>
        <div className="mt-1 flex flex-wrap items-center gap-x-1 text-3 text-ink-2">
          <span className="brand-type">{work.wait.length}</span> 件等你决定
          <span className="text-ink-3">·</span>
          <span className="brand-type">{problems.length}</span> 个要处理
          <span className="text-ink-3">·</span>
          <span className="brand-type">{advancing.length}</span> 件在推进
          <button
            type="button"
            data-kind="now-notify"
            onClick={() => setNotifyOpen((value) => !value)}
            className="ml-auto inline-flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2"
          >
            什么时候会通知你
            <ChevronDown className={cn('size-3 transition-transform', notifyOpen && 'rotate-180')} />
          </button>
        </div>
        {notifyOpen ? (
          <p data-kind="now-notify-body" className="mt-1 rounded-info border border-line bg-pane/50 px-2.5 py-1.5 text-1 leading-4 text-ink-2">
            等你决定和出问题会提醒；在推进的不一条条通知，完成或卡住才说。看过不算处理，事情解决了才离开这一页。
          </p>
        ) : null}

        {allEmpty ? (
          <p data-kind="now-empty" className="mt-8 text-3 text-ink-2">
            都在正常推进。
          </p>
        ) : (
          <>
            <section className="mt-6" data-kind="now-decisions">
              <GroupHeader tone="var(--wait)" label="等你决定" count={work.wait.length} hint="和工作页「等你」同一份" />
              {work.wait.length ? (
                <div className="space-y-1.5">
                  {work.wait.slice(0, shown('wait', work.wait.length)).map((item, index) => (
                    <WaitRow key={item.kind + ':' + (item.taskId ?? item.title) + ':' + String(index)} item={item} />
                  ))}
                </div>
              ) : (
                <p className="text-1 text-ink-3">没有要你决定的事。</p>
              )}
              <MoreButton hidden={Boolean(expanded['wait'])} more={work.wait.length - MAX_PER_GROUP} onClick={() => setExpanded((current) => ({ ...current, wait: true }))} />
            </section>

            <section className="mt-6" data-kind="now-problems">
              <GroupHeader tone="var(--fail)" label="出了问题" count={problems.length} hint="Kind 会先自己处理；处理不了的才会变成上面的决定" />
              {problems.length ? (
                <div className="space-y-1.5">
                  {problems.slice(0, shown('problems', problems.length)).map((task) => (
                    <ProblemRow key={task.id} task={task} />
                  ))}
                </div>
              ) : (
                <p className="text-1 text-ink-3">没有出问题。</p>
              )}
              <MoreButton hidden={Boolean(expanded['problems'])} more={problems.length - MAX_PER_GROUP} onClick={() => setExpanded((current) => ({ ...current, problems: true }))} />
            </section>

            <section className="mt-6" data-kind="now-advancing">
              <GroupHeader tone="var(--run)" label="在推进" count={advancing.length} />
              {cards.length ? (
                <div className="space-y-1.5">
                  {cards.slice(0, shown('advancing', cards.length)).map((card) => (
                    <AdvancingRow key={card.key} card={card} />
                  ))}
                </div>
              ) : (
                <p className="text-1 text-ink-3">没有在推进的任务。</p>
              )}
              <MoreButton hidden={Boolean(expanded['advancing'])} more={cards.length - MAX_PER_GROUP} onClick={() => setExpanded((current) => ({ ...current, advancing: true }))} />
            </section>
          </>
        )}

        <section className="mt-6 border-t border-line pt-3">
          <button
            type="button"
            data-kind="now-week-done"
            onClick={() => go('work')}
            className="flex w-full items-center gap-2 text-left text-2 text-ink-2 transition-colors hover:text-ink"
          >
            <span className="grid size-4 shrink-0 place-items-center rounded-full bg-done-soft text-1 text-done">✓</span>
            本周完成 <span className="brand-type">{doneThisWeek.count}</span> 件
            {doneThisWeek.names.length ? <>：{doneThisWeek.names.join('、')}</> : null}
            <span className="ml-auto shrink-0 text-1 text-ink-3">查看 ›</span>
          </button>
          <p className="mt-3 text-1 leading-4 text-ink-3">「还需」带 ≈ 的是估算：按这件事已完成的几站平均用时推算，不含等你的时间。</p>
          <p className="mt-1 text-1 leading-4 text-ink-3">
            AIOS 概念 · 现在：你不在时发生的事汇总在这里；问题默认由 Kind 先处理，只有要你拍板的才冒成「等你决定」。
          </p>
        </section>
      </div>
    </div>
  );
}
