/**
 * 项目页（#project:<id>；§4.5 里程碑 / §4.6 成果；brief §四）。
 *
 * 默认布局「项目页 | 对话（可收起）」（layout-motion-v1.md §一）：
 * - 左边：项目简报、里程碑（已通过的折成一行，进行中的展开看成功标准与任务）、成果；
 * - 右边：项目对话（原型是样例；「收起」把对话收成一条，第 3 期由 Dock 接管真正的收起）。
 * 数据只从 api 取（getProject / getTaskDetail 类似）。
 */

import { useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, Plus, SendHorizontal } from 'lucide-react';
import { toast } from 'sonner';
import { getProject } from '@/data/api';
import type { ProjectDetail } from '@/data/selectors';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { StartThingDialog } from '@/pages/work/StartThingDialog';
import { cn } from '@/lib/utils';

const STATE_LABEL: Record<string, string> = {
  todo: '排队',
  run: '在做',
  wait: '等你',
  fail: '出了问题',
  done: '完成',
};

const STATE_TONE: Record<string, string> = {
  todo: 'bg-hover text-ink-2',
  run: 'bg-run-soft text-run',
  wait: 'bg-wait-soft text-wait',
  fail: 'bg-fail-soft text-fail',
  done: 'bg-done-soft text-done',
};

const TYPE_GRADIENT = [
  'linear-gradient(135deg, #7c5cf5 0%, #c7b6ff 100%)',
  'linear-gradient(135deg, #1aa3ba 0%, #9fe3ee 100%)',
  'linear-gradient(135deg, #b9764a 0%, #f0c9a8 100%)',
  'linear-gradient(135deg, #4c8dff 0%, #b8d0ff 100%)',
];

function MilestoneSection({ milestone }: { milestone: ProjectDetail['milestones'][number] }) {
  const [open, setOpen] = useState(milestone.state !== 'passed');
  const passed = milestone.state === 'passed';
  return (
    <section className="rounded-card border border-line bg-pane/40">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="flex w-full flex-wrap items-center gap-2 px-3 py-2.5 text-left transition-colors hover:bg-hover"
      >
        {passed ? <span className="text-done">✓</span> : <span>🚩</span>}
        <span className="text-3 text-ink">{milestone.name}</span>
        <span className="rounded-full bg-hover px-2 py-[1px] text-1 text-ink-3">
          {passed ? '已通过' : milestone.state === 'judging' ? '判断中' : '进行中'}
        </span>
        {milestone.when ? <span className="text-1 text-ink-3">{milestone.when}</span> : null}
        {milestone.due ? <span className="text-1 text-ink-3">{milestone.due}</span> : null}
        <span className="ml-auto text-1 text-ink-3">{milestone.tasks.length} 件事</span>
        <ChevronRight className={cn('size-3.5 text-ink-3 transition-transform', open && 'rotate-90')} />
      </button>

      {open ? (
        <div className="border-t border-line px-3 py-2.5">
          {milestone.criteria.length ? (
            <ul className="space-y-1">
              {milestone.criteria.map(([text, state, evidence]) => (
                <li key={text} className="flex items-baseline gap-2 text-2">
                  <span className={cn('shrink-0', state === 'ok' ? 'text-done' : state === 'no' ? 'text-fail' : 'text-wait')}>
                    {state === 'ok' ? '✓' : state === 'no' ? '✗' : '○'}
                  </span>
                  <span className="text-ink-2">{text}</span>
                  {evidence ? <span className="min-w-0 truncate text-1 text-ink-3">{evidence}</span> : null}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-2 text-ink-3">这个里程碑没有列成功标准。</p>
          )}

          {milestone.tasks.length ? (
            <div className="mt-2.5 space-y-1.5">
              {milestone.tasks.map((task) => (
                <button
                  key={task.id}
                  type="button"
                  onClick={() => go('task:' + task.id)}
                  className="flex w-full items-center gap-2 rounded-info border border-line bg-content/40 px-2.5 py-2 text-left transition-colors hover:bg-hover"
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-2 text-ink">{task.title}</span>
                    <span className="mt-0.5 block truncate text-1 text-ink-3">
                      {task.role}
                      {task.note ? ' · ' + task.note : ''}
                    </span>
                    {task.progress ? (
                      <span className="mt-1 block h-[3px] w-40 overflow-hidden rounded-full bg-hover">
                        <span
                          className="block h-full rounded-full bg-run"
                          style={{ width: (task.progress.done / Math.max(task.progress.total, 1)) * 100 + '%' }}
                        />
                      </span>
                    ) : null}
                  </span>
                  <span className={cn('shrink-0 rounded-full px-2 py-[1px] text-1', STATE_TONE[task.state])}>
                    {STATE_LABEL[task.state]}
                  </span>
                  <ChevronRight className="size-3.5 shrink-0 text-ink-3" />
                </button>
              ))}
            </div>
          ) : (
            <p className="mt-2 text-2 text-ink-3">还没有任务。</p>
          )}
        </div>
      ) : null}
    </section>
  );
}

function ProjectMainPane() {
  const projectId = useRouteStore((state) => state.route.id);
  const [detail, setDetail] = useState<ProjectDetail | null | undefined>(undefined);
  const [startOpen, setStartOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    setDetail(undefined);
    void (async () => {
      const next = await getProject(projectId);
      if (alive) setDetail(next);
    })();
    return () => {
      alive = false;
    };
  }, [projectId]);

  if (detail === undefined) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取项目…</div>;
  }
  if (!detail) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">没有这个项目：{projectId}</div>;
  }

  return (
    <div className="h-full overflow-y-auto" data-scene-page="project">
      <div className="mx-auto w-full max-w-[860px] px-6 py-5">
        <div className="flex flex-wrap items-center gap-2">
          <span
            className="grid size-[18px] place-items-center rounded-badge brand-type text-[10px] font-semibold text-white"
            style={{ backgroundColor: detail.color }}
          >
            {detail.code}
          </span>
          <h1 className="text-5 font-semibold leading-7 text-ink">{detail.name}</h1>
          <span className="text-1 text-ink-3">
            完成 {detail.counts.done} / {detail.counts.total}
          </span>
          <button
            type="button"
            onClick={() => setStartOpen(true)}
            className="ml-auto flex h-7 items-center gap-1 rounded-btn bg-ink px-2.5 text-2 text-content transition-opacity hover:opacity-90"
          >
            <Plus className="size-3.5" />
            交给 Kind 一件事
          </button>
        </div>
        <p className="mt-1.5 max-w-[620px] text-2 leading-5 text-ink-2">{detail.brief}</p>

        <div className="mt-5 space-y-2">
          {detail.milestones.map((milestone) => (
            <MilestoneSection key={milestone.id} milestone={milestone} />
          ))}
        </div>

        <section className="mt-6">
          <h2 className="text-3 font-medium text-ink">成果</h2>
          {detail.artifacts.length ? (
            <div className="mt-2 grid grid-cols-1 gap-2 sm:grid-cols-2">
              {detail.artifacts.map((artifact, index) => (
                <div key={artifact.name + artifact.from} className="rounded-card border border-line bg-pane/40 p-2">
                  <div
                    className="grid h-[76px] place-items-center rounded-info text-1 text-white/90"
                    style={{ background: TYPE_GRADIENT[index % TYPE_GRADIENT.length] }}
                  >
                    {artifact.name.split('.').pop()?.toUpperCase()}
                  </div>
                  <div className="mt-1.5 truncate text-2 text-ink" title={artifact.name}>
                    {artifact.name}
                  </div>
                  <div className="mt-0.5 flex items-center justify-between text-1 text-ink-3">
                    <span>
                      <span className="brand-type">{artifact.version}</span> · {artifact.size}
                    </span>
                    <span className={artifact.status === 'acc' ? 'text-done' : artifact.status === 'pend' ? 'text-wait' : 'text-ink-3'}>
                      {artifact.status === 'acc' ? '✓✓ 已接受' : artifact.status === 'pend' ? '待你接受' : '草稿'}
                    </span>
                  </div>
                  <div className="mt-0.5 truncate text-1 text-ink-3">来自：{artifact.from}</div>
                </div>
              ))}
            </div>
          ) : (
            <p className="mt-2 text-2 text-ink-3">还没有成果。</p>
          )}
        </section>

        <section className="mt-6">
          <h2 className="text-3 font-medium text-ink">资料</h2>
          <p className="mt-0.5 text-1 text-ink-3">放在这个项目里的资料；还能查全局的 {detail.globalMaterials} 份。</p>
          {detail.materials.length ? (
            <div className="mt-2 overflow-hidden rounded-card border border-line">
              {detail.materials.map((material) => (
                <button
                  key={material.name}
                  type="button"
                  onClick={() => go('read:' + material.name)}
                  className="flex w-full items-center gap-2 border-b border-line px-3 py-2 text-left transition-colors last:border-0 hover:bg-hover"
                >
                  <span className="rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3">{material.type}</span>
                  <span className="min-w-0 flex-1 truncate text-2 text-ink-2">{material.name}</span>
                  <span className="brand-type text-1 text-ink-3">{material.version}</span>
                  <span className="rounded-full bg-hover px-2 py-[1px] text-1 text-ink-3">{material.mode}</span>
                  <span
                    className={cn(
                      'size-1.5 shrink-0 rounded-full',
                      material.status === 'ok' ? 'bg-done' : material.status === 'busy' ? 'bg-wait' : 'bg-fail',
                    )}
                  />
                </button>
              ))}
            </div>
          ) : (
            <p className="mt-2 text-2 text-ink-3">这个项目里还没有资料。</p>
          )}
        </section>

      </div>
      <StartThingDialog open={startOpen} onOpenChange={setStartOpen} />
    </div>
  );
}

function ProjectChatPane() {
  const projectId = useRouteStore((state) => state.route.id);
  const [detail, setDetail] = useState<ProjectDetail | null | undefined>(undefined);
  const [open, setOpen] = useState(true);

  useEffect(() => {
    let alive = true;
    void (async () => {
      const next = await getProject(projectId);
      if (alive) setDetail(next);
    })();
    return () => {
      alive = false;
    };
  }, [projectId]);

  if (!detail) {
    return (
      <div className="flex h-full items-start bg-content px-4 py-3 text-2 text-ink-3" data-pane="chat">
        {detail === undefined ? '正在取项目对话…' : '没有这个项目。'}
      </div>
    );
  }

  if (!open) {
    return (
      <div className="flex h-full flex-col items-center justify-start gap-2 bg-content px-3 py-3" data-pane="chat">
        <button
          type="button"
          onClick={() => setOpen(true)}
          className="flex w-full items-center gap-1.5 rounded-btn px-1.5 py-1 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
        >
          <ChevronDown className="size-3.5 rotate-90" />
          项目对话 · 展开
        </button>
      </div>
    );
  }

  const openMilestone = detail.milestones.find((milestone) => milestone.state !== 'passed');
  const nextTask = openMilestone?.tasks.find((task) => task.state !== 'done');
  const waitingTask = openMilestone?.tasks.find((task) => task.state === 'wait' || task.state === 'fail');

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-pane="chat">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-3">
        <span className="text-2 text-ink-2">项目对话</span>
        <span className="brand-type text-1 text-ink-3">{detail.code}</span>
        <button
          type="button"
          onClick={() => setOpen(false)}
          className="ml-auto rounded-btn px-1.5 py-0.5 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
        >
          收起
        </button>
      </header>

      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-3 py-3 text-2 leading-5">
        <div className="ml-auto max-w-[86%] rounded-card bg-hover px-2.5 py-2 text-ink">
          帮我把「{detail.name}」推进一下，先把「{openMilestone?.name ?? '下一件事'}」过掉。
        </div>
        <div className="max-w-[92%] text-ink-2">
          <span className="text-1 text-ink-3">Kind</span>
          <p className="mt-0.5">
            好。已经完成 {detail.counts.done} / {detail.counts.total} 项。
            {nextTask ? ' 下一站是「' + nextTask.title + '」，我接着做。' : ' 现在没有在跑的站。'}
          </p>
        </div>
        <div className="max-w-[92%] text-ink-2">
          <span className="text-1 text-ink-3">Kind</span>
          <p className="mt-0.5">
            {waitingTask
              ? '「' + waitingTask.title + '」要你先定一下；定完我再往下走。'
              : '没有要你定的事；做完我会把结果放到成果里。'}
          </p>
        </div>
      </div>

      <div className="shrink-0 border-t border-line px-3 py-2">
        <div className="flex items-end gap-1.5 rounded-btn border border-line-2 bg-pane px-2 py-1.5">
          <textarea
            rows={1}
            placeholder={'和 Kind 说这个项目的事…'}
            className="max-h-24 min-h-5 flex-1 resize-none bg-transparent text-2 text-ink outline-none placeholder:text-ink-3"
          />
          <button
            type="button"
            onClick={() => toast('（原型）项目对话只做样例；发送等第 3 期接真实会话')}
            className="grid size-6 shrink-0 place-items-center rounded-full bg-hover text-ink-3 transition-colors hover:text-ink-2"
            title="发送（原型）"
          >
            <SendHorizontal className="size-3.5" />
          </button>
        </div>
      </div>
    </div>
  );
}

export function ProjectScene({ pane }: { pane: string }) {
  if (pane === 'chat') return <ProjectChatPane />;
  return <ProjectMainPane />;
}
