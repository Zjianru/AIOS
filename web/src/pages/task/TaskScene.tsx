/**
 * 任务页（#task:<id>；§4.1 地铁线 / §4.2 节点层 / §4.4 这件事的账 / §4.5 里程碑；brief §四「工作」）。
 *
 * 左边：地铁线（点一站进下一层）、冒泡的等你事项、叶子任务的节点层、里程碑、成果；
 * 右边：这件事的账（含预计还需的依据）、问 Kind 这件事、控制这件事（暂停 / 取消）。
 * 数据只从 api 取（getTaskDetail）。
 */

import { useEffect, useState } from 'react';
import { go } from '@/app/navigate';
import { getTaskDetail } from '@/data/api';
import type { TaskDetail } from '@/data/selectors';
import type { TaskState } from '@/data/fixtures/workspace';
import { toast } from 'sonner';
import { mark } from '@/lib/perf';
import { TaskLine } from '@/components/subway/TaskLine';
import { useRouteStore } from '@/app/routeStore';
import { cn } from '@/lib/utils';

const STATE_LABEL: Record<TaskState, string> = {
  todo: '排队',
  run: '在做',
  wait: '等你',
  fail: '出了问题',
  done: '完成',
};

const STATE_PILL: Record<TaskState, string> = {
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

function Ledger({ detail }: { detail: TaskDetail }) {
  const rows: { label: string; value: string; note?: string }[] = [
    { label: '模型', value: detail.ledger.model },
    { label: '已用时间', value: detail.ledger.elapsed },
    { label: '预计还需', value: detail.ledger.eta },
    { label: 'Token', value: detail.ledger.tokens },
    { label: '费用', value: detail.ledger.cost },
  ];
  return (
    <section className="rounded-card border border-line bg-pane/40 p-3">
      <div className="flex items-baseline gap-2">
        <h3 className="text-3 text-ink">这件事的账</h3>
        <span className="text-1 text-ink-3">含全部子任务</span>
      </div>
      <dl className="mt-2 space-y-1">
        {rows.map((row) => (
          <div key={row.label} className="flex items-baseline justify-between gap-2 text-2">
            <dt className="text-ink-3">{row.label}</dt>
            <dd className="brand-type text-ink-2">{row.value}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-2 text-1 leading-4 text-ink-3">{detail.ledger.etaNote}</p>
      <button
        type="button"
        onClick={() => go('usage:' + detail.id)}
        className="mt-2 text-1 text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2"
      >
        用量与提醒 ›
      </button>
    </section>
  );
}

function ControlCard({ detail }: { detail: TaskDetail }) {
  const running = detail.children.find((child) => child.state === 'run');
  const count = detail.children.length || 1;
  return (
    <section className="rounded-card border border-line bg-pane/40 p-3">
      <h3 className="text-3 text-ink">控制这件事</h3>
      <div className="mt-2 flex gap-2">
        <button
          type="button"
          onClick={() => toast('（原型）已暂停「' + detail.title + '」；随时能恢复')}
          className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
        >
          暂停这件事
        </button>
        <button
          type="button"
          onClick={() => toast('（原型）已取消后续工作；已经交出的成果留着')}
          className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
        >
          取消
        </button>
      </div>
      <p className="mt-2 text-1 leading-4 text-ink-3">
        暂停：不再开始新的一步。{running ? '正在做的「' + running.title + '」先停在一个安全的地方；' : ''}已有成果和记录都留着，随时能恢复。
      </p>
      <p className="mt-1.5 text-1 leading-4 text-ink-3">取消：正在做的会停下收尾；已经交出的成果留着。取消后不能恢复。</p>
      <p className="mt-1.5 text-1 leading-4 text-ink-3">
        {detail.children.length ? '管这件事和它下面的 ' + count + ' 项，' : '只管这一件，'}不影响别的任务，不删除成果。
      </p>
    </section>
  );
}

function NodeLayer({ detail }: { detail: TaskDetail }) {
  if (!detail.nodeLayer) return null;
  return (
    <section className="mt-5">
      <h2 className="text-3 text-ink">怎么做的 · {detail.nodeLayer.tier}</h2>
      <div className="mt-2 space-y-1.5">
        {detail.nodeLayer.steps.map((step) => (
          <div key={step.title} className="flex items-center gap-2 rounded-card border border-line bg-pane/40 px-3 py-2">
            <span className="size-[7px] rounded-full bg-done" />
            <span className="text-2 text-ink">{step.title}</span>
            <span className="text-1 text-ink-3">{step.who}</span>
            <span className="ml-auto truncate text-1 text-ink-3">{step.result}</span>
          </div>
        ))}
      </div>
      {detail.nodeLayer.handover.length ? (
        <div className="mt-3">
          <div className="text-1 text-ink-3">交接记录</div>
          <ul className="mt-1 space-y-0.5">
            {detail.nodeLayer.handover.map((entry) => (
              <li key={entry.at + entry.what} className="flex gap-2 text-1 text-ink-3">
                <span className="brand-type shrink-0">{entry.at}</span>
                <span className="shrink-0">{entry.who}</span>
                <span className="min-w-0 truncate text-ink-2">{entry.what}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      <p className="mt-2 text-1 text-ink-3">{detail.nodeLayer.machineCheck}</p>
    </section>
  );
}

function MilestoneBlock({ detail }: { detail: TaskDetail }) {
  if (!detail.milestone) return null;
  return (
    <section className="mt-5">
      <h2 className="text-3 text-ink">
        🚩 里程碑 · {detail.milestone.name}
        <span className="ml-2 text-1 text-ink-3">
          {detail.milestone.state}
          {detail.milestone.due ? ' · ' + detail.milestone.due : ''}
        </span>
      </h2>
      <ul className="mt-1.5 space-y-1">
        {detail.milestone.criteria.map(([text, state, evidence]) => (
          <li key={text} className="flex items-baseline gap-2 text-2">
            <span className={cn('shrink-0', state === 'ok' ? 'text-done' : state === 'no' ? 'text-fail' : 'text-wait')}>
              {state === 'ok' ? '✓' : state === 'no' ? '✗' : '○'}
            </span>
            <span className="text-ink-2">{text}</span>
            {evidence ? <span className="truncate text-1 text-ink-3">{evidence}</span> : null}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Artifacts({ detail }: { detail: TaskDetail }) {
  if (!detail.artifacts.length) return null;
  return (
    <section className="mt-5">
      <h2 className="text-3 text-ink">成果</h2>
      <div className="mt-2 grid grid-cols-1 gap-2 sm:grid-cols-2">
        {detail.artifacts.map((artifact, index) => (
          <div key={artifact.name} className="rounded-card border border-line bg-pane/40 p-2">
            <div
              className="grid h-[92px] place-items-center rounded-info text-1 text-white/90"
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
          </div>
        ))}
      </div>
    </section>
  );
}

export function TaskScene() {
  // 面板内容由 dockview 自己渲染；同一个场景内换路由必须订阅 routeStore，否则停在旧任务上。
  const taskId = useRouteStore((state) => state.route.id);
  const [detail, setDetail] = useState<TaskDetail | null | undefined>(undefined);

  useEffect(() => {
    let alive = true;
    setDetail(undefined);
    void (async () => {
      const next = await getTaskDetail(taskId);
      if (!alive) return;
      setDetail(next);
      // 第 2 期测速：数据到位 + 两帧 rAF 之后算「详情页打开」的终点。
      requestAnimationFrame(() => requestAnimationFrame(() => mark('task:data-painted')));
    })();
    return () => {
      alive = false;
    };
  }, [taskId]);

  if (detail === undefined) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">正在取任务…</div>;
  }
  if (!detail) {
    return <div className="grid h-full place-items-center text-2 text-ink-3">没有这个任务：{taskId}</div>;
  }

  const attention = detail.attention;

  return (
    <div className="h-full overflow-y-auto" data-scene-page="task">
      <div className="mx-auto w-full max-w-[1080px] px-6 py-5">
        <div className="flex flex-wrap items-center gap-1 text-1 text-ink-3">
          <span>{detail.projectName}</span>
          {detail.path.slice(0, -1).map((entry) => (
            <span key={entry.id} className="flex items-center gap-1">
              <span aria-hidden="true">›</span>
              <span className="truncate">{entry.title}</span>
            </span>
          ))}
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-2">
          <h1 className="text-6 font-semibold leading-8 text-ink">{detail.title}</h1>
          <span className={cn('rounded-full px-2 py-[1px] text-1', STATE_PILL[detail.state])}>{STATE_LABEL[detail.state]}</span>
        </div>
        {detail.children.length ? (
          <p className="mt-0.5 text-2 text-ink-3">
            {detail.children.length} 项子任务 · 点一站进入下一层
          </p>
        ) : (
          <p className="mt-0.5 text-2 text-ink-3">叶子任务 · 节点层</p>
        )}

        <div className="mt-4 flex items-start gap-5">
          <div className="min-w-0 flex-1">
            {detail.children.length ? <TaskLine detail={detail} /> : null}

            {attention ? (
              <button
                type="button"
                onClick={() => go('task:' + attention.id)}
                className="mt-3 flex w-full items-center gap-2 rounded-card border border-wait/30 bg-wait-soft/40 px-3 py-2 text-left"
              >
                <span className="text-2 text-ink">{attention.title}</span>
                <span className="text-1 text-ink-3">
                  {attention.state === 'wait' ? '：等你决定' : '：出了问题'}
                </span>
                <span className="ml-auto text-1 text-ink-3">去看 ›</span>
              </button>
            ) : null}

            <NodeLayer detail={detail} />
            <div className="mt-2 flex justify-end">
              <button
                type="button"
                onClick={() => go('trace:' + detail.id)}
                className="text-1 text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2"
              >
                原始记录（第三层）›
              </button>
            </div>
            <MilestoneBlock detail={detail} />
            <Artifacts detail={detail} />
          </div>

          <aside className="w-[320px] shrink-0 space-y-3">
            <Ledger detail={detail} />
            <button
              type="button"
              onClick={() => go('kind')}
              className="w-full rounded-card border border-line bg-pane/40 px-3 py-2 text-left text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
            >
              问 Kind 这件事 ›
            </button>
            <ControlCard detail={detail} />
          </aside>
        </div>

        <p className="mt-6 border-t border-line pt-2 text-1 leading-4 text-ink-3">
          AIOS 概念 · 任务页：地铁线是任务图（点一站进下一层，看到的还是同样的地铁线）；右边是「这件事的账」和它自己的控制开关。
        </p>
      </div>
    </div>
  );
}
