/**
 * 工作页「任务图」：同一份 WorkResponse 事实画谁等谁（只读）。
 *
 * - 每张已确认图一块；站用看板的 column/word/next 和颜色，线用服务给的 kind/state/program 拼固定词；
 * - 实线 = 下游开跑时拿到了；虚线 = 还在等；等你判/需修正用自己的颜色；不画动效；
 * - 点站：前后一步亮起来，右栏五样（要取得什么/做到哪了/下一步/等谁/谁等它），能打开任务页；
 * - 每张图可切「表」，同样的事实一行一个任务；单派任务只在看板，页尾只报数量。
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowUpRight, FilePen, Flag, SearchCheck, Table2, Terminal } from 'lucide-react';
import { go } from '@/app/navigate';
import { taskRead } from '@/data/service';
import type { TaskReadResponse } from '@/data/generated/TaskReadResponse';
import { cn } from '@/lib/utils';
import { getElk } from '@/lib/elk';
import type { ElkNode } from 'elkjs/lib/elk-api';
import { deriveGraphs, dotOf, neighbours } from './graphModel';
import type { GraphEdge, GraphNode, TaskGraph } from './graphModel';
import { formatAt, WAIT_LABEL } from './workModel';
import { useWorkStore } from './workStore';

const NODE_W = 196;
const NODE_H = 74;

const EDGE_TONE: Record<GraphEdge['state'], string> = {
  handed: 'var(--ink-2)',
  waiting: 'var(--ink-3)',
  judge: 'var(--wait)',
  fix: 'var(--fail)',
};

interface Point {
  x: number;
  y: number;
}
interface Layout {
  width: number;
  height: number;
  nodes: Record<string, Point>;
  edges: Record<string, Point[]>;
}

async function layoutGraph(graph: TaskGraph): Promise<Layout> {
  const engine = await getElk();
  const input: ElkNode = {
    id: graph.id,
    layoutOptions: {
      'elk.algorithm': 'layered',
      'elk.direction': 'RIGHT',
      'elk.layered.spacing.nodeNodeBetweenLayers': '56',
      'elk.spacing.nodeNode': '20',
      'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF',
      'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
      'elk.edgeRouting': 'ORTHOGONAL',
      'elk.padding': '[top=12,left=12,bottom=12,right=12]',
    },
    children: graph.nodes.map((node) => ({ id: node.task.id, width: NODE_W, height: NODE_H })),
    edges: graph.edges.map((edge) => ({ id: edge.id, sources: [edge.from], targets: [edge.to] })),
  };
  const result = await engine.layout(input);
  const nodes: Record<string, Point> = {};
  for (const child of result.children ?? []) nodes[child.id] = { x: child.x ?? 0, y: child.y ?? 0 };
  const edges: Record<string, Point[]> = {};
  for (const edge of result.edges ?? []) {
    const section = edge.sections?.[0];
    if (section) edges[edge.id] = [section.startPoint, ...(section.bendPoints ?? []), section.endPoint];
  }
  return { width: result.width ?? 0, height: result.height ?? 0, nodes, edges };
}

/** 图的形状变了才重排；状态变了不动位置。 */
const shapeOf = (graph: TaskGraph) =>
  graph.nodes.map((node) => node.task.id).join(',') +
  '|' +
  graph.edges.map((edge) => edge.id).join(',');

function useLayout(graph: TaskGraph): Layout | null {
  const shape = shapeOf(graph);
  const latest = useRef(graph);
  latest.current = graph;
  const [layout, setLayout] = useState<{ shape: string; layout: Layout } | null>(null);
  useEffect(() => {
    let live = true;
    void layoutGraph(latest.current).then((next) => {
      if (live) setLayout({ shape, layout: next });
    });
    return () => {
      live = false;
    };
    // 只按形状重排：状态每两秒刷新，位置不动。
  }, [shape]);
  return layout?.shape === shape ? layout.layout : null;
}

function kindWord(task: GraphNode['task']): { icon: typeof Flag; text: string } {
  if (task.program) {
    return { icon: Terminal, text: '程序门 · ' + String(task.programCommands) + ' 条命令' };
  }
  if (task.gate) return { icon: Flag, text: '里程碑 · 只读' };
  if (task.writeScope) return { icon: FilePen, text: '写 ' + task.writeScope };
  return { icon: SearchCheck, text: '只读' };
}

function Station({
  node,
  at,
  selected,
  dim,
  onSelect,
}: {
  node: GraphNode;
  at: Point;
  selected: boolean;
  dim: boolean;
  onSelect: () => void;
}) {
  const { task, tile } = node;
  const kind = kindWord(task);
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'absolute rounded-card border bg-raised p-2 text-left transition-[opacity,border-color]',
        selected ? 'border-ink-2 shadow-sm' : 'border-line hover:border-ink-3/50',
        dim && 'opacity-40',
      )}
      style={{ left: at.x, top: at.y, width: NODE_W, height: NODE_H }}
      data-kind="graph-node"
      data-node-id={task.id}
      data-title={task.title}
      data-column={tile.column}
      data-selected={selected ? 'true' : 'false'}
      data-dim={dim ? 'true' : 'false'}
      aria-pressed={selected}
    >
      <span className="flex min-w-0 items-center gap-1 text-1 text-ink-3">
        <kind.icon className="size-3 shrink-0" />
        <span className="truncate">{kind.text}</span>
        <span className="ml-auto flex shrink-0 items-center gap-1">
          <span className="size-[7px] rounded-full" style={{ backgroundColor: dotOf(tile.column) }} />
          {tile.word}
        </span>
      </span>
      <span className="mt-0.5 block truncate text-2 text-ink" title={task.title}>
        {task.title}
      </span>
      <span className="mt-0.5 flex items-center justify-between gap-2 text-1 text-ink-3">
        <span className="truncate">{tile.column === 'done' ? '关了' : '下一步：' + tile.next}</span>
        <span className="shrink-0">{task.program ? '程序' : task.version > 1 ? '第 ' + String(task.version) + ' 版' : '任务'}</span>
      </span>
    </button>
  );
}

function Wire({
  edge,
  points,
  lit,
  dim,
}: {
  edge: GraphEdge;
  points: Point[];
  lit: boolean;
  dim: boolean;
}) {
  const d = points.map((point, index) => (index ? 'L' : 'M') + String(point.x) + ' ' + String(point.y)).join(' ');
  const end = points[points.length - 1];
  const before = points[points.length - 2] ?? end;
  const stroke = EDGE_TONE[edge.state];
  const dir = end.x >= before.x ? 1 : -1;
  return (
    <g
      opacity={dim ? 0.25 : 1}
      data-kind="graph-edge"
      data-state={edge.state}
      data-edge-kind={edge.kind}
      data-edge-id={edge.id}
      data-from={edge.from}
      data-to={edge.to}
      data-lit={lit ? 'true' : 'false'}
      data-dim={dim ? 'true' : 'false'}
    >
      <title>{edge.label}</title>
      <path
        d={d}
        fill="none"
        stroke={stroke}
        strokeWidth={lit ? 2.5 : 1.75}
        strokeDasharray={edge.state === 'handed' ? undefined : '5 4'}
      />
      <path
        d={'M' + String(end.x - 6 * dir) + ' ' + String(end.y - 4) + ' L' + String(end.x) + ' ' + String(end.y) + ' L' + String(end.x - 6 * dir) + ' ' + String(end.y + 4)}
        fill="none"
        stroke={stroke}
        strokeWidth={1.75}
      />
    </g>
  );
}

function GraphCanvas({
  graph,
  selected,
  onSelect,
}: {
  graph: TaskGraph;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const layout = useLayout(graph);
  const near = selected && graph.nodes.some((node) => node.task.id === selected) ? neighbours(graph, selected) : null;
  if (!layout) return <div className="h-24 animate-pulse rounded-card bg-pane/40" />;
  return (
    <div className="overflow-x-auto pb-1">
      <div className="relative" style={{ width: layout.width, height: layout.height }}>
        <svg className="absolute inset-0" width={layout.width} height={layout.height} aria-hidden>
          {graph.edges.map((edge) =>
            layout.edges[edge.id] ? (
              <Wire
                key={edge.id}
                edge={edge}
                points={layout.edges[edge.id]}
                lit={Boolean(near?.edges.has(edge.id))}
                dim={Boolean(near && !near.edges.has(edge.id))}
              />
            ) : null,
          )}
        </svg>
        {graph.nodes.map((node) =>
          layout.nodes[node.task.id] ? (
            <Station
              key={node.task.id}
              node={node}
              at={layout.nodes[node.task.id]}
              selected={selected === node.task.id}
              dim={Boolean(near && !near.nodes.has(node.task.id))}
              onSelect={() => onSelect(node.task.id)}
            />
          ) : null,
        )}
      </div>
    </div>
  );
}

function GraphTable({
  graph,
  selected,
  onSelect,
}: {
  graph: TaskGraph;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const rows = [...graph.nodes].sort((a, b) => a.depth - b.depth || a.row - b.row);
  const titleOf = (id: string) => graph.nodes.find((node) => node.task.id === id)?.task.title ?? '';
  return (
    <table className="w-full text-2" data-kind="graph-table" data-graph-id={graph.id}>
      <thead>
        <tr className="border-b border-line text-left text-1 text-ink-3">
          <th className="py-1.5 pr-3 font-normal">任务</th>
          <th className="py-1.5 pr-3 font-normal">状态</th>
          <th className="py-1.5 pr-3 font-normal">下一步</th>
          <th className="py-1.5 pr-3 font-normal">等谁</th>
          <th className="py-1.5 font-normal">谁等它</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((node) => {
          const needs = graph.edges.filter((edge) => edge.to === node.task.id && edge.kind !== 'release');
          const releases = graph.edges.filter((edge) => edge.from === node.task.id && edge.kind === 'release');
          const waiting = graph.edges.filter(
            (edge) => edge.to === node.task.id && edge.kind === 'release',
          );
          const upstream = [...needs, ...waiting];
          return (
            <tr
              key={node.task.id}
              data-kind="graph-row"
              data-node-id={node.task.id}
              className={cn('border-b border-line/60 cursor-pointer hover:bg-hover', selected === node.task.id && 'bg-hover')}
              onClick={() => onSelect(node.task.id)}
            >
              <td className="py-1.5 pr-3 text-ink">{node.task.title}</td>
              <td className="py-1.5 pr-3 text-ink-3">
                {node.tile.word}
                {node.task.gate ? ' · 门' : ''}
              </td>
              <td className="py-1.5 pr-3 text-ink-3">{node.tile.next}</td>
              <td className="py-1.5 pr-3 text-ink-3">{upstream.map((edge) => titleOf(edge.from)).join('、') || '—'}</td>
              <td className="py-1.5 text-ink-3">
                {[...releases].map((edge) => titleOf(edge.to)).join('、') || '—'}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function DetailValue({ label, children, kind }: { label: string; children: React.ReactNode; kind: string }) {
  return (
    <div className="mt-2" data-detail={kind}>
      <div className="text-1 text-ink-3">{label}</div>
      <div className="mt-0.5 text-2 leading-5 text-ink-2">{children}</div>
    </div>
  );
}

function Detail({ graph, id }: { graph: TaskGraph; id: string }) {
  const node = graph.nodes.find((item) => item.task.id === id);
  const [read, setRead] = useState<TaskReadResponse | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let live = true;
    setRead(null);
    setError('');
    void taskRead(id)
      .then((response) => {
        if (live) setRead(response);
      })
      .catch((reason: unknown) => {
        if (live) setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => {
      live = false;
    };
  }, [id]);
  if (!node) return null;
  const titleOf = (taskId: string) => graph.nodes.find((item) => item.task.id === taskId)?.task.title ?? taskId;
  const needs = graph.edges.filter((edge) => edge.to === id);
  const waited = graph.edges.filter((edge) => edge.from === id && edge.kind === 'release');
  return (
    <aside className="w-[360px] shrink-0 overflow-y-auto border-l border-line bg-pane/30 px-4 py-3" data-kind="graph-detail" data-node-id={id}>
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-3 font-medium text-ink">{node.task.title}</div>
          <div className="mt-0.5 text-1 text-ink-3">
            {node.tile.word} · {node.task.gate ? '里程碑门' : node.task.program ? '程序门' : node.task.writeScope ? '写任务' : '调查任务'}
          </div>
        </div>
        <button
          type="button"
          data-kind="graph-open-task"
          onClick={() => go('taskchat:' + id)}
          className="flex shrink-0 items-center gap-1 rounded-btn border border-line px-2 py-1 text-1 text-ink-2 hover:bg-hover"
        >
          打开任务页 <ArrowUpRight className="size-3" />
        </button>
      </div>
      <DetailValue label="要取得什么" kind="requirement">
        {read ? read.task.requirement : error ? '读不到要求：' + error : '正在读…'}
      </DetailValue>
      <DetailValue label="做到哪了" kind="progress">
        {node.task.latest
          ? node.task.latest.summary
          : '第 ' + String(node.task.attempts) + ' 次尝试，还没有报告'}
        {node.task.latest ? ' · ' + (WAIT_LABEL[graph.edges.find((edge) => edge.to === id && edge.state === 'judge')?.state ?? ''] ?? '') : ''}
      </DetailValue>
      <DetailValue label="下一步谁来" kind="next">
        {node.tile.next}
      </DetailValue>
      <DetailValue label="等谁" kind="upstream">
        {needs.length === 0
          ? '没有前置'
          : needs.map((edge) => titleOf(edge.from) + '（' + edge.label + '）').join('；')}
      </DetailValue>
      <DetailValue label="谁等它" kind="downstream">
        {waited.length === 0 ? '没有下游' : waited.map((edge) => titleOf(edge.to)).join('、')}
      </DetailValue>
      <div className="mt-3 text-1 text-ink-3">
        {node.task.acceptedAt ? '你在 ' + formatAt(node.task.acceptedAt) + ' 接受了结果' : '只读：关系由服务算，页面不在这里改'}
      </div>
    </aside>
  );
}

function GraphBlock({
  graph,
  selected,
  onSelect,
}: {
  graph: TaskGraph;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const [table, setTable] = useState(false);
  const counts = graph.counts;
  return (
    <section className="rounded-card border border-line bg-pane/20 p-3" data-kind="task-graph" data-graph-id={graph.id} data-closed={graph.closed ? 'true' : 'false'}>
      <header className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-3 font-medium text-ink">{graph.title}</div>
          <div className="mt-0.5 text-1 text-ink-3">
            {graph.nodes.length} 个任务 · 排队 {counts.queue} · 在做 {counts.doing} · 等你 {counts.wait} · 完成 {counts.done}
            {graph.closed ? ' · 已全部关闭' : ''}
          </div>
        </div>
        <button
          type="button"
          data-kind="graph-table-toggle"
          onClick={() => setTable((value) => !value)}
          className="flex shrink-0 items-center gap-1 rounded-btn border border-line px-2 py-1 text-1 text-ink-2 hover:bg-hover"
        >
          <Table2 className="size-3" />
          {table ? '看图' : '看表'}
        </button>
      </header>
      <div className="mt-2">
        {table ? (
          <GraphTable graph={graph} selected={selected} onSelect={onSelect} />
        ) : (
          <GraphCanvas graph={graph} selected={selected} onSelect={onSelect} />
        )}
      </div>
    </section>
  );
}

export function GraphPane({
  selected,
  onSelect,
}: {
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  // 轮询由 WorkScene 的 BoardPane 统一持有；这里只读同一份 store，不再另起 interval。
  const work = useWorkStore((state) => state.work);
  const error = useWorkStore((state) => state.error);
  const { graphs, single } = useMemo(() => deriveGraphs(work), [work]);
  const owner = graphs.find((graph) => graph.nodes.some((node) => node.task.id === selected));
  if (!work) {
    return (
      <div className="grid h-full place-items-center text-2 text-ink-3">
        {error ?? '正在取工作页…'}
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 overflow-hidden" data-kind="task-graph-pane">
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {graphs.length === 0 ? (
          <div className="grid h-full place-items-center text-2 text-ink-3" data-kind="task-graph-empty">
            现在没有已确认的任务图；Kind 把一件事拆成几步、你确认了，这里就有一张。
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {graphs.map((graph) => (
              <GraphBlock key={graph.id} graph={graph} selected={selected} onSelect={onSelect} />
            ))}
          </div>
        )}
        <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1 px-1 text-1 text-ink-3">
          <span className="inline-flex items-center gap-1">
            <svg width="22" height="6" aria-hidden><path d="M0 3 H22" stroke="var(--ink-2)" strokeWidth="1.75" /></svg>
            下游开跑时拿到了
          </span>
          <span className="inline-flex items-center gap-1">
            <svg width="22" height="6" aria-hidden><path d="M0 3 H22" stroke="var(--ink-3)" strokeWidth="1.75" strokeDasharray="5 4" /></svg>
            还在等
          </span>
          <span className="inline-flex items-center gap-1">
            <svg width="22" height="6" aria-hidden><path d="M0 3 H22" stroke="var(--wait)" strokeWidth="1.75" strokeDasharray="5 4" /></svg>
            等你判
          </span>
          {single > 0 ? (
            <span className="ml-auto" data-kind="graph-single-count">另有 {single} 个单派的任务，没有前后关系，在看板里</span>
          ) : null}
        </div>
      </div>
      {owner && selected ? <Detail graph={owner} id={selected} /> : null}
    </div>
  );
}
