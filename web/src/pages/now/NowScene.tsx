/**
 * 首页「现在」（experience-spec-v1.md §3；brief §四）。
 *
 * 从上到下：等你决定（暖橙）→ 出了问题（红；分 LLM / AIOS，只陈述「发生了什么 → Kind 怎么处理」，
 * 不放决定按钮）→ 在推进（蓝；一个根任务一张卡，卡里一条地铁线）→ 本周完成一行。
 * 卡片和地铁线用的都是同一套 token；「还需」带 ≈ 的写明是估算。
 * 数据只从 api 取（getNow），不 import 样例。
 */

import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { getNow } from '@/data/api';
import type { NowDecision, NowFixture, NowProblem, NowRootCard, NowUsage } from '@/data/fixtures/now';
import { nowCounts } from '@/data/fixtures/now';
import { go } from '@/app/navigate';
import { LineRow } from '@/components/subway/LineRow';
import { cn } from '@/lib/utils';

const TONE: Record<NowDecision['kind'], string> = {
  askDecision: 'text-wait',
  artifact: 'text-done',
  askWeb: 'text-wait',
  assignRole: 'text-ink-3',
  milestone: 'text-wait',
};

const HANDLED: Record<NowProblem['handled'], string> = {
  已处理: 'text-done',
  处理中: 'text-fail',
  已转为决定: 'text-wait',
};

const HANDLED_DOT: Record<NowProblem['handled'], string> = {
  已处理: 'var(--done)',
  处理中: 'var(--fail)',
  已转为决定: 'var(--wait)',
};

const SIDE: Record<NowProblem['side'], string> = { llm: 'LLM 问题', aios: 'AIOS 问题' };

function CodeChip({ code, color }: { code: string | null; color?: string }) {
  if (!code) return null;
  return (
    <span
      className="grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white"
      style={{ backgroundColor: color ?? 'var(--ink-3)' }}
    >
      {code}
    </span>
  );
}

function Origin({ code, project, task, color }: { code: string | null; project: string | null; task: string | null; color?: string }) {
  if (!project) return null;
  return (
    <span className="flex min-w-0 items-center gap-1 text-1 text-ink-3">
      <CodeChip code={code} color={color} />
      <span className="truncate">{project}</span>
      {task ? <span className="truncate opacity-80"> › {task}</span> : null}
    </span>
  );
}

/** 等你决定的卡；工作页的「等你」列也复用它（内容与首页「等你决定」相同）。 */
export function DecisionCard({ item }: { item: NowDecision }) {
  const act = (label: string) => toast('（原型）' + label + '：' + item.title);
  return (
    <article className="rounded-card border border-line bg-pane/60 p-3">
      <header className="flex flex-wrap items-center gap-2 text-1 text-ink-3">
        <span className={cn('font-medium', TONE[item.kind])}>{item.typeLabel}</span>
        {item.fromProblem ? (
          <span className="rounded-badge bg-wait-soft px-1.5 py-[1px] text-wait">因问题而来</span>
        ) : null}
        <Origin code={item.projectCode} project={item.projectName} task={item.taskTitle} />
        <span className="ml-auto brand-type">{item.time}</span>
      </header>

      <h3 className="mt-1.5 text-4 font-medium leading-6 text-ink">{item.title}</h3>
      {item.artifact ? (
        <div className="mt-1 flex items-baseline gap-2 text-2">
          <span className="text-ink-2">{item.artifact.name}</span>
          <span className="brand-type text-ink-3">{item.artifact.version}</span>
        </div>
      ) : null}
      {item.why ? <p className="mt-1 text-2 leading-5 text-ink-2">{item.why}</p> : null}

      {item.options?.length ? (
        <div className="mt-2 space-y-1">
          <div className="text-1 text-ink-3">Kind 给的选项 · 点一行就是选它</div>
          {item.options.map((option) => (
            <button
              key={option}
              type="button"
              onClick={() => act(option)}
              className="flex w-full items-center gap-2 rounded-lg border border-line px-2.5 py-1.5 text-left transition-colors hover:bg-hover"
            >
              <span className="size-3 shrink-0 rounded-full border border-ink-3" />
              <span className="text-3 text-ink">{option}</span>
            </button>
          ))}
          <button
            type="button"
            onClick={() => act('我自己说')}
            className="w-full rounded-lg border border-dashed border-line-2 px-2.5 py-1.5 text-left text-3 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
          >
            我自己说…
          </button>
        </div>
      ) : null}

      <div className="mt-2 flex items-center gap-2">
        {item.actions.map((action) => (
          <button
            key={action.label}
            type="button"
            onClick={() => act(action.label)}
            className={cn(
              'rounded-btn px-2.5 py-1 text-2 transition-colors',
              action.primary ? 'bg-ink text-content hover:opacity-90' : 'border border-line-2 text-ink-2 hover:bg-hover',
            )}
          >
            {action.label}
          </button>
        ))}
        <button
          type="button"
          onClick={() => go(item.href)}
          className="ml-auto flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2"
        >
          展开 · 去看 <span aria-hidden="true">›</span>
        </button>
      </div>
    </article>
  );
}

function ProblemRow({ item }: { item: NowProblem }) {
  return (
    <article className="rounded-card border border-line bg-pane/40 p-3">
      <header className="flex flex-wrap items-center gap-2 text-1 text-ink-3">
        <span className="text-fail">出了问题</span>
        <Origin code={null} project={item.projectName} task={item.taskTitle} />
        <span className="rounded-badge bg-hover px-1.5 py-[1px]">{SIDE[item.side]}</span>
        <span className={cn('flex items-center gap-1 rounded-badge bg-hover px-1.5 py-[1px]', HANDLED[item.handled])}>
          <span className="size-[6px] rounded-full" style={{ backgroundColor: HANDLED_DOT[item.handled] }} />
          {item.handled}
        </span>
        <span className="ml-auto brand-type">{item.time}</span>
      </header>
      <div className="mt-1 text-3 text-ink">{item.what}</div>
      <div className="mt-0.5 text-2 text-ink-3">Kind 的处理：{item.how}</div>
      <div className="mt-1 flex justify-end">
        <button
          type="button"
          onClick={() => (item.href ? go(item.href) : toast('（原型）这是 AIOS 自己的问题，Kind 正在处理'))}
          className="flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2"
        >
          展开 · 去看 <span aria-hidden="true">›</span>
        </button>
      </div>
    </article>
  );
}

const FLAG_TONE: Record<NonNullable<NowRootCard['flag']>['tone'], string> = {
  wait: 'bg-wait-soft text-wait',
  fail: 'bg-fail-soft text-fail',
  run: 'bg-run-soft text-run',
};

function RootCard({ root, usage }: { root: NowRootCard; usage: NowUsage }) {
  return (
    <article className="rounded-card border border-line bg-pane/60 p-3">
      <header className="flex flex-wrap items-center gap-2">
        <h3 className="text-4 font-medium text-ink">{root.title}</h3>
        {root.subtitle ? <span className="text-1 text-ink-3">{root.subtitle}</span> : null}
        <CodeChip code={root.projectCode} color={root.color} />
        <span className="text-2 text-ink-2">{root.projectName}</span>
        {root.flag ? (
          <span className={cn('ml-auto rounded-full px-2 py-[1px] text-1', FLAG_TONE[root.flag.tone])}>{root.flag.text}</span>
        ) : null}
      </header>

      <div className="mt-2.5">
        <LineRow color={root.color} stations={root.line} onGo={(href) => go(href)} />
      </div>

      {root.attention ? (
        <button
          type="button"
          onClick={() => go('now')}
          className="mt-2 flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2"
        >
          <span aria-hidden="true">↳</span> {root.attention} <span aria-hidden="true">›</span>
        </button>
      ) : null}

      <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-1 text-ink-3">
        <span className="text-ink-2">{usage.model}</span>
        <span className="opacity-60">·</span>
        <span>
          已用 <span className="brand-type">{usage.elapsed}</span>
        </span>
        <span className="opacity-60">·</span>
        <span>
          还需 <span className="brand-type">{usage.eta}</span>
        </span>
        <span className="opacity-60">·</span>
        <span className="brand-type">{usage.tokens} token</span>
        <span className="opacity-60">·</span>
        <span className="brand-type">{usage.cost}</span>
        <button
          type="button"
          onClick={() => go('project:' + root.projectId)}
          className="ml-auto flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2"
        >
          展开 · 去看 <span aria-hidden="true">›</span>
        </button>
      </div>
    </article>
  );
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

function greetingOf(hour: number): string {
  if (hour < 6) return '夜深了';
  if (hour < 11) return '早上好';
  if (hour < 13) return '中午好';
  if (hour < 18) return '下午好';
  return '晚上好';
}

export function NowScene() {
  const [data, setData] = useState<NowFixture | null>(null);

  useEffect(() => {
    let alive = true;
    void (async () => {
      const next = await getNow();
      if (alive) setData(next);
    })();
    return () => {
      alive = false;
    };
  }, []);

  if (!data) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取「现在」…</div>;
  }

  const counts = nowCounts(data);
  const greeting = greetingOf(new Date().getHours());

  return (
    <div className="h-full overflow-y-auto" data-scene-page="now">
      <div className="mx-auto w-full max-w-[860px] px-6 py-6">
        <h1 className="text-6 font-semibold leading-8 text-ink">{greeting}</h1>
        <p className="mt-1 flex flex-wrap items-center gap-x-1 text-3 text-ink-2">
          <span className="brand-type">{counts.decisions}</span> 件等你决定
          <span className="text-ink-3">·</span>
          <span className="brand-type">{counts.problems}</span> 个问题（Kind 已处理 <span className="brand-type">{counts.fixed}</span> 个）
          <span className="text-ink-3">·</span>
          <span className="brand-type">{counts.running}</span> 件在推进
          <button
            type="button"
            onClick={() => go('notify')}
            className="ml-2 text-1 text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2"
          >
            什么时候会通知你
          </button>
        </p>

        {data.decisions.length ? (
          <section className="mt-6">
            <GroupHeader tone="var(--wait)" label="等你决定" count={data.decisions.length} />
            <div className="space-y-2">
              {data.decisions.map((item) => (
                <DecisionCard key={item.id} item={item} />
              ))}
            </div>
          </section>
        ) : null}

        {data.problems.length ? (
          <section className="mt-6">
            <GroupHeader
              tone="var(--fail)"
              label="出了问题"
              count={data.problems.length}
              hint="Kind 会先自己处理；处理不了的才会变成上面的决定"
            />
            <div className="space-y-2">
              {data.problems.map((item) => (
                <ProblemRow key={item.id} item={item} />
              ))}
            </div>
          </section>
        ) : null}

        {data.roots.length ? (
          <section className="mt-6">
            <GroupHeader tone="var(--run)" label="在推进" count={data.roots.length} />
            <div className="space-y-2">
              {data.roots.map((root) => (
                <RootCard key={root.id} root={root} usage={data.usage[root.projectId]} />
              ))}
            </div>
          </section>
        ) : null}

        <section className="mt-6 border-t border-line pt-3">
          <button
            type="button"
            onClick={() => go('work')}
            className="flex w-full items-center gap-2 text-left text-2 text-ink-2 transition-colors hover:text-ink"
          >
            <span className="grid size-4 shrink-0 place-items-center rounded-full bg-done-soft text-1 text-done">✓</span>
            本周完成 <span className="brand-type">{data.weekDone.count}</span> 件：{data.weekDone.names.join('、')}
            <span className="ml-auto shrink-0 text-1 text-ink-3">查看 ›</span>
          </button>
          <p className="mt-3 text-1 leading-4 text-ink-3">
            「还需」带 ≈ 的是估算：按这件事已完成的几站平均用时推算，不含等你的时间。
          </p>
        </section>
      </div>
    </div>
  );
}
