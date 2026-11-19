/**
 * 工作页「任务图」的展示派生：事实来自 WorkResponse.tasks + graphs。
 *
 * 谁等谁、哪条线判没判由服务算好；这里只做展示：站用看板同一份 column/word/next，
 * 线拼固定词、给邻接和排版列号。不重算门/交接的业务规则。
 */

import type { TaskFact } from '@/data/generated/TaskFact';
import type { WorkEdgeFact } from '@/data/generated/WorkEdgeFact';
import type { WorkResponse } from '@/data/generated/WorkResponse';
import { boardOf, columnOf, nextOf, wordOf } from './workModel';
import type { BoardTask } from './workModel';

export interface GraphNode {
  task: TaskFact;
  tile: BoardTask;
  /** 从左往右第几列。 */
  depth: number;
  /** 这一列里第几个。 */
  row: number;
  kind: 'task' | 'milestone' | 'program';
}

export type EdgeState = 'handed' | 'waiting' | 'judge' | 'fix';

export interface GraphEdge {
  id: string;
  from: string;
  to: string;
  kind: 'needs' | 'guards' | 'release';
  state: EdgeState;
  program: boolean;
  /** 线上那句话（固定词；详情里展开）。 */
  label: string;
  /** flow / make / back / judge / 无。 */
  live: string | null;
  /** 线上小字（第 N 份 · X 文件 / 打回第 N 份）。 */
  payload: string | null;
}

export interface TaskGraph {
  id: string;
  title: string;
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** 全部子任务都关了。 */
  closed: boolean;
  counts: { queue: number; doing: number; wait: number; done: number };
}

function labelOf(edge: WorkEdgeFact, from: TaskFact | undefined): string {
  switch (edge.kind) {
    case 'needs':
      if (edge.state === 'handed') {
        const times = Math.max(1, from?.attempts ?? 1);
        return '交了第 ' + String(times) + ' 次的结果';
      }
      return from && from.latest ? '有结果了，等开跑' : '等它有结果';
    case 'guards':
      return edge.program ? '每交一份，程序跑一次命令' : '每交一份，核一次';
    case 'release':
      if (edge.state === 'handed') return '交了判通过的那份';
      if (edge.state === 'judge') return '等你判';
      if (edge.state === 'fix') return '判了需修正';
      return '判通过才交给它';
    default:
      return edge.kind;
  }
}

function tileOf(
  task: TaskFact,
  waitTaskIds: Set<string>,
  waitKindOf: Map<string, string>,
): BoardTask {
  const column = columnOf(task, waitTaskIds);
  return {
    task,
    column,
    word: wordOf(task, column, waitKindOf.get(task.id)),
    next: nextOf(task, column),
  };
}

/** 列：最左是没有上游的站，每条线往右一列。图很小，直接按最长前驱链算。 */
function depths(nodes: GraphNode[], edges: GraphEdge[]): Map<string, number> {
  const incoming = new Map<string, string[]>();
  for (const edge of edges) {
    incoming.set(edge.to, (incoming.get(edge.to) ?? []).concat(edge.from));
  }
  const depth = new Map<string, number>();
  const visit = (id: string, seen: Set<string>): number => {
    const cached = depth.get(id);
    if (cached !== undefined) return cached;
    const parents = incoming.get(id) ?? [];
    if (parents.length === 0) {
      depth.set(id, 0);
      return 0;
    }
    if (seen.has(id)) return 0;
    seen.add(id);
    let next = 0;
    for (const parent of parents) next = Math.max(next, visit(parent, seen) + 1);
    seen.delete(id);
    depth.set(id, next);
    return next;
  };
  for (const node of nodes) visit(node.task.id, new Set());
  return depth;
}

/** WorkResponse 里的 graphs → 页面模型；单派任务只数数量。 */
export function deriveGraphs(work: WorkResponse | null): { graphs: TaskGraph[]; single: number } {
  if (!work) return { graphs: [], single: 0 };
  const tasks = new Map(work.tasks.map((task) => [task.id, task]));
  const waitTaskIds = new Set<string>();
  const waitKindOf = new Map<string, string>();
  for (const item of work.wait) {
    if (item.taskId && !waitTaskIds.has(item.taskId)) waitTaskIds.add(item.taskId);
    if (item.taskId && !waitKindOf.has(item.taskId)) waitKindOf.set(item.taskId, item.kind);
  }
  const graphs: TaskGraph[] = [];
  for (const source of work.graphs) {
    const members: TaskFact[] = [];
    for (const edge of source.edges) {
      for (const id of [edge.from, edge.to]) {
        const task = tasks.get(id);
        if (task && !members.some((item) => item.id === id)) members.push(task);
      }
    }
    members.sort((a, b) => a.createdAt - b.createdAt || a.title.localeCompare(b.title));
    const edges: GraphEdge[] = source.edges.map((edge) => ({
      id: edge.from + '>' + edge.to + ':' + edge.kind,
      from: edge.from,
      to: edge.to,
      kind: edge.kind as GraphEdge['kind'],
      state: edge.state as EdgeState,
      program: edge.program,
      live: edge.live ?? null,
      payload: edge.payload ?? null,
      label: labelOf(edge, tasks.get(edge.from)),
    }));
    const nodes: GraphNode[] = members.map((task) => ({
      task,
      tile: tileOf(task, waitTaskIds, waitKindOf),
      depth: 0,
      row: 0,
      kind: task.program ? 'program' : task.gate ? 'milestone' : 'task',
    }));
    const depth = depths(nodes, edges);
    const rows = new Map<number, number>();
    for (const node of nodes) {
      const column = depth.get(node.task.id) ?? 0;
      node.depth = column;
      node.row = rows.get(column) ?? 0;
      rows.set(column, node.row + 1);
    }
    const counts = { queue: 0, doing: 0, wait: 0, done: 0 };
    for (const node of nodes) counts[node.tile.column] += 1;
    graphs.push({
      id: source.id,
      title: source.title,
      nodes,
      edges,
      closed: nodes.length > 0 && counts.done === nodes.length,
      counts,
    });
  }
  // 没关完的在上；同样的按最早的子任务排。
  graphs.sort(
    (left, right) =>
      Number(left.closed) - Number(right.closed) ||
      (left.nodes[0]?.task.createdAt ?? 0) - (right.nodes[0]?.task.createdAt ?? 0),
  );
  const single = work.tasks.filter((task) => !task.parent).length;
  return { graphs, single };
}

/** 选中一个节点：前后各一步的节点和线。 */
export function neighbours(graph: TaskGraph, id: string): { nodes: Set<string>; edges: Set<string> } {
  const nodes = new Set([id]);
  const edges = new Set<string>();
  for (const edge of graph.edges) {
    if (edge.from === id || edge.to === id) {
      edges.add(edge.id);
      nodes.add(edge.from);
      nodes.add(edge.to);
    }
  }
  return { nodes, edges };
}

/** 这一列的状态点颜色（和看板一致）。 */
export function dotOf(column: BoardTask['column']): string {
  if (column === 'doing') return 'var(--run)';
  if (column === 'wait') return 'var(--wait)';
  if (column === 'done') return 'var(--done)';
  return 'var(--ink-3)';
}

export { boardOf };
