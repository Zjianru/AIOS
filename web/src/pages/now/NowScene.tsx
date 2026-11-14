/**
 * 首页「现在」（T055；D96 三组都出）。
 *
 * 等你决定 = 工作页同一份 Work.wait；出了问题 = 待补足条件、不在「等你」里的任务（行上「处理中」）；
 * 在推进 = 看板「在做」列的任务。每组最多 3 条，其余「还有 N 条」点开；
 * 三组都没有写一句「都在正常推进」。最下面一行「本周完成」从已关闭的任务算。
 */

import { useEffect, useMemo, useState } from 'react';
import { useWorkPolling, useWorkStore } from '@/pages/work/workStore';
import { WaitRow } from '@/pages/work/WaitList';
import { advancingOf, boardOf, problemsOf, stageLabel, weekDone } from '@/pages/work/workModel';
import type { TaskFact } from '@/data/generated/TaskFact';
import { go } from '@/app/navigate';

const MAX_PER_GROUP = 3;

function greetingOf(hour: number): string {
  if (hour < 6) return '夜深了';
  if (hour < 11) return '早上好';
  if (hour < 13) return '中午好';
  if (hour < 18) return '下午好';
  return '晚上好';
}

function GroupHeader({
  tone,
  label,
  count,
  hint,
}: {
  tone: string;
  label: string;
  count: number;
  hint?: string;
}) {
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

function ProblemRow({ task }: { task: TaskFact }) {
  return (
    <button
      type="button"
      data-kind="now-problem"
      onClick={() => go('taskchat:' + task.id)}
      className="block w-full rounded-card border border-line bg-pane/40 p-2.5 text-left transition-colors hover:bg-hover"
    >
      <span className="flex flex-wrap items-center gap-1.5 text-1 text-ink-3">
        <span className="text-fail">出了问题</span>
        <span className="rounded-badge bg-hover px-1.5 py-[1px]">处理中</span>
        <span className="rounded-badge bg-hover px-1.5 py-[1px]">{stageLabel(task.stage)}</span>
        <span className="ml-auto brand-type">{task.attempts > 0 ? '第 ' + String(task.attempts) + ' 次' : ''}</span>
      </span>
      <span className="mt-0.5 block truncate text-2 text-ink">{task.title}</span>
      <span className="mt-0.5 block truncate text-1 text-ink-3">
        {task.latest?.question ?? task.latest?.summary ?? 'Kind 正在补条件。'}
      </span>
    </button>
  );
}

function AdvancingRow({ task }: { task: TaskFact }) {
  const card = {
    task,
    column: 'doing' as const,
    word: task.stage === 'running' ? '执行中' : '检查中',
    next: 'ThinAgent',
  };
  return (
    <button
      type="button"
      data-kind="now-advancing"
      onClick={() => go('taskchat:' + task.id)}
      className="block w-full rounded-card border border-line bg-pane/40 p-2.5 text-left transition-colors hover:bg-hover"
    >
      <span className="flex flex-wrap items-center gap-1.5 text-1 text-ink-3">
        <span className="text-run">{card.word}</span>
        <span className="ml-auto brand-type">下一步：{card.next}</span>
      </span>
      <span className="mt-0.5 block truncate text-2 text-ink">{card.task.title}</span>
      <span className="block truncate text-1 text-ink-3">
        {card.task.writeScope ? '写入范围 ' + card.task.writeScope : '只读调查'}
      </span>
    </button>
  );
}

export function NowScene() {
  useWorkPolling();
  const work = useWorkStore((state) => state.work);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    setNow(Date.now());
  }, [work]);

  const board = useMemo(() => boardOf(work?.tasks ?? [], work?.wait ?? []), [work]);
  const problems = useMemo(
    () => problemsOf(work?.tasks ?? [], work?.wait ?? []),
    [work],
  );
  const advancing = useMemo(() => advancingOf(board), [board]);
  const doneThisWeek = useMemo(() => weekDone(work?.tasks ?? [], now), [work, now]);

  if (!work) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取「现在」…</div>;
  }

  const allEmpty = work.wait.length === 0 && problems.length === 0 && advancing.length === 0;
  const greeting = greetingOf(new Date().getHours());
  const shown = (key: string, total: number): number =>
    expanded[key] ? total : Math.min(total, MAX_PER_GROUP);

  return (
    <div className="h-full overflow-y-auto" data-scene-page="now">
      <div className="mx-auto w-full max-w-[860px] px-6 py-6">
        <h1 className="text-6 font-semibold leading-8 text-ink">{greeting}</h1>
        <p className="mt-1 flex flex-wrap items-center gap-x-1 text-3 text-ink-2">
          <span className="brand-type">{work.wait.length}</span> 件等你决定
          <span className="text-ink-3">·</span>
          <span className="brand-type">{problems.length}</span> 个要处理
          <span className="text-ink-3">·</span>
          <span className="brand-type">{advancing.length}</span> 件在推进
        </p>

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
              <MoreButton
                hidden={Boolean(expanded['wait'])}
                more={work.wait.length - MAX_PER_GROUP}
                onClick={() => setExpanded((current) => ({ ...current, wait: true }))}
              />
            </section>

            <section className="mt-6" data-kind="now-problems">
              <GroupHeader
                tone="var(--fail)"
                label="出了问题"
                count={problems.length}
                hint="Kind 会先自己处理；处理不了的才会变成上面的决定"
              />
              {problems.length ? (
                <div className="space-y-1.5">
                  {problems.slice(0, shown('problems', problems.length)).map((task) => (
                    <ProblemRow key={task.id} task={task} />
                  ))}
                </div>
              ) : (
                <p className="text-1 text-ink-3">没有出问题。</p>
              )}
              <MoreButton
                hidden={Boolean(expanded['problems'])}
                more={problems.length - MAX_PER_GROUP}
                onClick={() => setExpanded((current) => ({ ...current, problems: true }))}
              />
            </section>

            <section className="mt-6" data-kind="now-advancing">
              <GroupHeader tone="var(--run)" label="在推进" count={advancing.length} />
              {advancing.length ? (
                <div className="space-y-1.5">
                  {advancing.slice(0, shown('advancing', advancing.length)).map((task) => (
                    <AdvancingRow key={task.id} task={task} />
                  ))}
                </div>
              ) : (
                <p className="text-1 text-ink-3">没有在推进的任务。</p>
              )}
              <MoreButton
                hidden={Boolean(expanded['advancing'])}
                more={advancing.length - MAX_PER_GROUP}
                onClick={() => setExpanded((current) => ({ ...current, advancing: true }))}
              />
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
        </section>
      </div>
    </div>
  );
}
