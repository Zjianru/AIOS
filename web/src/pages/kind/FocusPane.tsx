/**
 * Kind 右栏「聚焦」：等你 / 出了问题 / 在跑三组。
 *
 * 数据用产品现有事实：工作快照（work.wait / problemsOf / tasks.running）+ graphModel.deriveGraphs
 * 的节点深度算「第几站」。站进度线只表达现在停在第几站，不编进度百分比（内核没有）。
 */

import { useMemo } from 'react';
import { go } from '@/app/navigate';
import { useProjectStore } from '@/pages/project/projectStore';
import { deriveGraphs } from '@/pages/work/graphModel';
import { useWorkPolling, useWorkStore } from '@/pages/work/workStore';
import { problemsOf, stageLabel } from '@/pages/work/workModel';
import type { TaskFact } from '@/data/generated/TaskFact';
import type { WorkResponse } from '@/data/generated/WorkResponse';

type Tone = 'wait' | 'fail' | 'run';

interface FocusRow {
  key: string;
  title: string;
  project: string;
  station: number;
  total: number;
  tone: Tone;
  hint: string;
  taskId?: string;
}

const TONE: Record<Tone, string> = {
  wait: 'var(--wait)',
  fail: 'var(--fail)',
  run: 'var(--run)',
};

function StationLine({ station, total, tone }: { station: number; total: number; tone: Tone }) {
  const count = Math.min(Math.max(total, 1), 6);
  return (
    <div className="mt-1 flex items-center gap-[2px]" aria-hidden="true">
      {Array.from({ length: count }, (_, index) => (
        <span
          key={index}
          className="h-[2px] flex-1 rounded-full"
          style={{
            backgroundColor:
              index < station - 1 ? 'var(--line-2)' : index === station - 1 ? TONE[tone] : 'var(--line)',
          }}
        />
      ))}
    </div>
  );
}

function Row({ row }: { row: FocusRow }) {
  return (
    <button
      type="button"
      data-focus-item={row.tone}
      onClick={() => {
        if (row.taskId) go('task:' + row.taskId);
      }}
      className="w-full border-b border-line/50 px-4 py-2 text-left last:border-0 hover:bg-hover"
    >
      <span className="flex items-baseline gap-1.5 text-1 text-ink-3">
        <span className="size-[6px] shrink-0 translate-y-[-1px] rounded-full" style={{ backgroundColor: TONE[row.tone] }} />
        <span className="truncate">{row.project}</span>
        <span className="shrink-0">· 第 {row.station} 站</span>
        <span className="ml-auto shrink-0">{row.hint}</span>
      </span>
      <span className="mt-0.5 block truncate text-2 text-ink">{row.title}</span>
      <StationLine station={row.station} total={row.total} tone={row.tone} />
    </button>
  );
}

function Group({ title, rows, empty }: { title: string; rows: FocusRow[]; empty: string }) {
  return (
    <section data-focus-group={title}>
      <h4 className="flex items-center gap-1.5 border-b border-line/60 px-4 py-1.5 text-1 text-ink-3">
        {title}
        <span className="brand-type">{rows.length}</span>
      </h4>
      {rows.length > 0 ? rows.map((row) => <Row key={row.key} row={row} />) : (
        <p className="px-4 py-2 text-1 text-ink-3">{empty}</p>
      )}
    </section>
  );
}

export function FocusPane() {
  useWorkPolling();
  const work = useWorkStore((state) => state.work);
  const projects = useProjectStore((state) => state.projects);

  const { waitRows, failRows, runRows, busy, running } = useMemo(() => {
    const projectName = (projectId: string | null | undefined) =>
      projects?.find((project) => project.id === projectId)?.name ?? '个人';
    const stations = new Map<string, { station: number; total: number }>();
    const graphs = deriveGraphs(work);
    for (const graph of graphs.graphs) {
      const total = Math.max(0, ...graph.nodes.map((node) => node.depth)) + 1;
      for (const node of graph.nodes) stations.set(node.task.id, { station: node.depth + 1, total });
    }
    const stationOf = (taskId: string | null | undefined) =>
      (taskId ? stations.get(taskId) : undefined) ?? { station: 1, total: 1 };
    const toRow = (task: TaskFact | null | undefined, wait: WorkResponse['wait'][number] | null, tone: Tone): FocusRow => {
      const at = stationOf(task?.id ?? wait?.taskId);
      return {
        key: (task?.id ?? wait?.taskId ?? wait?.title ?? '') + ':' + tone,
        title: task?.title ?? wait?.title ?? '',
        project: projectName(task?.projectId),
        station: at.station,
        total: at.total,
        tone,
        hint: tone === 'run' ? '在跑' : tone === 'fail' ? stageLabel(task?.stage ?? '') : (wait?.kind === 'accept' ? '等你接受' : '等你'),
        taskId: task?.id ?? wait?.taskId ?? undefined,
      };
    };
    const tasks = work?.tasks ?? [];
    const wait = work?.wait ?? [];
    const taskById = new Map(tasks.map((task) => [task.id, task]));
    const waitRows = wait.map((item) => toRow(item.taskId ? taskById.get(item.taskId) : null, item, 'wait'));
    const failRows = problemsOf(tasks, wait).map((task) => toRow(task, null, 'fail'));
    const runRows = tasks.filter((task) => task.running).map((task) => toRow(task, null, 'run'));
    return {
      waitRows,
      failRows,
      runRows,
      busy: waitRows.length + runRows.length,
      running: runRows.length,
    };
  }, [work, projects]);

  const subtitle = waitRows.length + failRows.length === 0 && runRows.length === 0
    ? '都在正常推进'
    : '要你看的 ' + (waitRows.length + failRows.length) + ' 件 · 在跑 ' + running + ' 件' + (busy > 0 ? '' : ' · 其余正常');

  return (
    <section className="flex min-h-0 max-h-[46%] shrink-0 flex-col overflow-hidden border-b border-line" data-block="focus">
      <header className="shrink-0 px-4 pb-1 pt-3">
        <h3 className="text-3 font-medium text-ink">聚焦</h3>
        <p className="mt-0.5 text-1 text-ink-3">{subtitle}</p>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <Group title="等你" rows={waitRows} empty="没有要你的事。" />
        <Group title="出了问题" rows={failRows} empty="没有出问题。" />
        <Group title="在跑" rows={runRows} empty="没有在跑。" />
      </div>
    </section>
  );
}
