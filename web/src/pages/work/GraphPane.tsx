/**
 * 工作页「任务图」（10 篇 5.2）：照原型 app-v14 b1ead1e 的 GraphPane 做全，只读。
 *
 * 卡三行（序号·类型 / 标题 / 状态点+看板原状态词）；写任务左边一道写的颜色，里程碑右上小旗，程序门直角等宽字。
 * 线只来自事实：实线=下游开跑时拿到了，虚线=还在等；在流的线有方向动效，减少动效时中间留方向箭头；线上写第几份。
 * 点卡右侧 320px 详情栏；可缩放/拖动/适合窗口；多图下拉切；空图报单派数。
 * 位置由程序排（最长路径分列 + 重心 4 轮 + 竖直居中 + 跨列 lane），不能拖，不在这里改关系。
 */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { ArrowLeft, ArrowRight, ArrowUpRight, ChevronDown, FilePen, Flag, Minus, Plus, SearchCheck, Terminal, X } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useSystemReducedMotion } from '@/app/useSystemReducedMotion';
import { go } from '@/app/navigate';
import { taskRead } from '@/data/service';
import type { TaskReadResponse } from '@/data/generated/TaskReadResponse';
import { cn } from '@/lib/utils';
import { useWorkStore } from './workStore';
import { COL_GAP, NODE_H, NODE_W, PAD, deriveGraphs, dotOf, layoutGraph, neighbours, shapeOf } from './graphModel';
import type { GraphEdge, GraphNode, GraphPoint, RoutedEdge, TaskGraph } from './graphModel';

const LINE_COLORS = ['var(--line-w)', 'var(--line-n)', 'var(--line-d)'];

/* ───────── 卡和线 ───────── */

type Shape = 'read' | 'write' | 'milestone' | 'program';

const shapeOfTask = (task: GraphNode['task']): Shape => (task.program ? 'program' : task.gate ? 'milestone' : task.writeScope ? 'write' : 'read');

function kindWord(task: GraphNode['task']): { icon: typeof Flag; text: string } {
  if (task.program) return { icon: Terminal, text: '程序门' };
  if (task.gate) return { icon: Flag, text: '里程碑' };
  if (task.writeScope) return { icon: FilePen, text: '写 ' + task.writeScope };
  return { icon: SearchCheck, text: '只读' };
}

function useReduced(): boolean {
  return useSystemReducedMotion();
}

const pad2 = (n: number) => String(n).padStart(2, '0');

function Station({ node, at, compact, selected, dim, onSelect }: { node: GraphNode; at: GraphPoint & { order: number }; compact: boolean; selected: boolean; dim: boolean; onSelect: () => void }) {
  const { task, tile } = node;
  const shape = shapeOfTask(task);
  const kind = kindWord(task);
  const dot = dotOf(tile.column);
  return (
    <button
      type="button"
      onClick={onSelect}
      title={task.title}
      className={cn(
        'graph-box absolute flex flex-col justify-between border border-line bg-content px-3.5 py-2.5 text-left shadow-sm transition-[opacity,border-color] hover:border-ink-3',
        selected && 'outline outline-2 outline-offset-2 outline-ink',
        dim && 'opacity-40',
        shape === 'program' && 'font-mono',
      )}
      style={{ left: at.x, top: at.y, width: NODE_W, height: NODE_H }}
      data-kind="graph-node"
      data-shape={shape}
      data-box={shape === 'milestone' ? 'gate' : shape}
      data-compact={compact ? 'true' : 'false'}
      data-column={tile.column}
      data-node-id={task.id}
      data-title={task.title}
      data-selected={selected ? 'true' : 'false'}
      data-dim={dim ? 'true' : 'false'}
      aria-pressed={selected}
    >
      {shape === 'write' ? <span aria-hidden className="graph-write-bar" /> : null}
      {shape === 'milestone' ? <span aria-hidden className="graph-flag" /> : null}
      {compact ? (
        <>
          <span className="flex items-center gap-2 text-[22px] leading-none text-ink-2">
            <span className="size-3 rounded-full" style={{ backgroundColor: dot }} />
            {pad2(at.order)}
          </span>
          <strong className="line-clamp-2 text-[22px] font-medium leading-tight text-ink">{task.title}</strong>
        </>
      ) : (
        <>
          <span className={cn('flex min-w-0 items-center gap-1 text-[12px]', shape === 'write' ? 'text-[var(--kind-write)]' : 'text-ink-3', shape === 'program' && 'font-mono')}>
            <span className="brand-type">{pad2(at.order)}</span>
            <span>·</span>
            <kind.icon className="size-3 shrink-0" />
            <span className="truncate">{kind.text}</span>
          </span>
          <strong className="line-clamp-2 text-[15px] font-medium leading-snug text-ink">{task.title}</strong>
          <span className="flex items-center gap-1.5 text-[12px] text-ink-2">
            <span className="size-[7px] rounded-full" style={{ backgroundColor: dot }} />
            {tile.word}
          </span>
        </>
      )}
    </button>
  );
}

function Wire({ edge, route, color, lit, dim, reduced }: { edge: GraphEdge; route: RoutedEdge; color: string; lit: boolean; dim: boolean; reduced: boolean }) {
  const live = edge.live;
  const moving = live === 'flow' || live === 'back';
  const motion = reduced ? undefined : moving ? (live === 'back' ? 'graph-dash-back' : 'graph-dash-flow') : live === 'make' ? 'graph-dash-out' : undefined;
  const width = moving ? 2.5 : lit ? 2 : 1.5;
  const { end, mid } = route;
  const k = live === 'back' ? -1 : 1;
  return (
    <g opacity={dim ? 0.25 : edge.state === 'handed' || live ? 1 : 0.6} data-kind="graph-edge" data-state={edge.state} data-live={live ?? ''} data-edge-id={edge.id} data-from={edge.from} data-to={edge.to}>
      <title>{edge.label + (edge.payload ? '：' + edge.payload : '')}</title>
      <path d={route.d} fill="none" stroke={color} strokeWidth={width} strokeLinecap={moving ? 'round' : 'butt'} strokeDasharray={moving ? '6 5' : edge.state === 'handed' ? undefined : '5 4'} className={motion} />
      <path d={'M' + String(end.x - 6) + ',' + String(end.y - 4) + ' L' + String(end.x) + ',' + String(end.y) + ' L' + String(end.x - 6) + ',' + String(end.y + 4)} fill="none" stroke={color} strokeWidth={1.75} />
      {reduced && moving ? <path data-kind="graph-reduced-arrow" d={'M' + String(mid.x - 4 * k) + ',' + String(mid.y - 4) + ' L' + String(mid.x + 4 * k) + ',' + String(mid.y) + ' L' + String(mid.x - 4 * k) + ',' + String(mid.y + 4)} fill="none" stroke={color} strokeWidth={2} /> : null}
    </g>
  );
}

/** 线上小字：流的是哪一份。 */
function Payload({ edge, route, dim }: { edge: GraphEdge; route: RoutedEdge; dim: boolean }) {
  if (!edge.payload) return null;
  return (
    <span
      className={cn('brand-type pointer-events-none absolute -translate-x-1/2 -translate-y-full whitespace-nowrap rounded-badge border border-line bg-content px-1.5 py-px text-1 leading-4 text-ink-2 transition-opacity', dim && 'opacity-25')}
      style={{ left: route.mid.x, top: route.mid.y - 3 }}
      data-kind="graph-payload"
    >
      {edge.live === 'back' ? <span className="text-[var(--fail)]">打回 · </span> : null}
      {edge.payload}
    </span>
  );
}

/* ───────── 选中的任务 ───────── */

const STAGE_TEXT: Record<string, string> = {
  needs_conditions: '待补条件',
  pending: '排队',
  running: '执行中',
  ready_for_review: '待检查',
  requirement_met: '已完成',
  closed: '已关闭',
};

function titleOf(graph: TaskGraph, id: string): string {
  return graph.nodes.find((node) => node.task.id === id)?.task.title ?? id;
}

function Detail({ graph, id, onClose }: { graph: TaskGraph; id: string; onClose: () => void }) {
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
  const { task, tile } = node;
  const into = graph.edges.filter((edge) => edge.to === id);
  const out = graph.edges.filter((edge) => edge.from === id);
  const progress = task.latest
    ? '第 ' + String(Math.max(1, task.attempts)) + ' 次 Attempt 交了结果：' + task.latest.summary
    : task.running
      ? '第 ' + String(Math.max(1, task.attempts)) + ' 次 Attempt 在跑，还没交结果'
      : '还没开跑';
  const dot = dotOf(tile.column);
  return (
    <aside className="flex min-h-0 flex-col border-l border-line bg-content" data-pane="graph-detail" data-kind="graph-detail" data-node-id={id}>
      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-4 py-3 text-2">
        <div>
          <div className="flex items-center gap-1.5 text-1 text-ink-3">
            <span className="size-[7px] rounded-full" style={{ backgroundColor: dot }} />
            {tile.word} · {STAGE_TEXT[task.stage] ?? task.stage}
            {task.version > 1 ? ' · 第 ' + String(task.version) + ' 版要求' : ''}
            <button type="button" onClick={onClose} title="关闭（Esc）" aria-label="关闭" className="-mr-1.5 ml-auto grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink" data-kind="graph-detail-close">
              <X className="size-3.5" />
            </button>
          </div>
          <h3 className="mt-1 text-3 font-medium text-ink">{task.title}</h3>
          <div className="mt-0.5 text-1 text-ink-3">{graph.title}</div>
        </div>
        <section>
          <h4 className="text-1 text-ink-3">要取得什么</h4>
          <p className="mt-0.5 leading-5 text-ink-2" data-detail="requirement">
            {read ? read.task.requirement : error ? '读不到要求：' + error : '正在读…'}
          </p>
        </section>
        <section>
          <h4 className="text-1 text-ink-3">做到哪了</h4>
          <p className="mt-0.5 leading-5 text-ink-2" data-detail="progress">{progress}</p>
        </section>
        <section>
          <h4 className="text-1 text-ink-3">下一步谁来</h4>
          <p className="mt-0.5 text-ink-2" data-detail="next">{tile.column === 'done' ? '没有了，已关闭' : tile.next}</p>
        </section>
        <section>
          <h4 className="text-1 text-ink-3">等谁</h4>
          {into.length ? (
            <ul className="mt-0.5 space-y-0.5 text-ink-2" data-detail="upstream">
              {into.map((edge) => (
                <li key={edge.id}>「{titleOf(graph, edge.from)}」：{edge.label}{edge.payload ? '（' + edge.payload + '）' : ''}</li>
              ))}
            </ul>
          ) : (
            <p className="mt-0.5 text-ink-3" data-detail="upstream">不等谁，确认图以后就能开跑</p>
          )}
        </section>
        <section>
          <h4 className="text-1 text-ink-3">谁等它</h4>
          {out.length ? (
            <ul className="mt-0.5 space-y-0.5 text-ink-2" data-detail="downstream">
              {out.map((edge) => (
                <li key={edge.id}>「{titleOf(graph, edge.to)}」：{edge.label}{edge.payload ? '（' + edge.payload + '）' : ''}</li>
              ))}
            </ul>
          ) : (
            <p className="mt-0.5 text-ink-3" data-detail="downstream">没有</p>
          )}
        </section>
      </div>
      <footer className="shrink-0 border-t border-line px-4 py-2">
        <button type="button" data-kind="graph-open-task" onClick={() => go('taskchat:' + task.id)} className="flex h-7 items-center gap-1 rounded-btn border border-line px-2.5 text-2 text-ink-2 hover:bg-hover hover:text-ink">
          打开任务页 <ArrowUpRight className="size-3.5" />
        </button>
        <p className="mt-1.5 text-1 leading-4 text-ink-3">要求、结果、判定、交付记录都在任务页。这里只看关系，不改。</p>
      </footer>
    </aside>
  );
}

/* ───────── 一张图的画布 ───────── */

const MIN_ZOOM = 0.3;
const READABLE = 0.5;
const MAX_ZOOM = 1.5;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function Canvas({ graph, color, selected, onSelect, header }: { graph: TaskGraph; color: string; selected: string | null; onSelect: (id: string | null) => void; header: ReactNode }) {
  const shapeKey = shapeOf(graph);
  // 只按形状重排
  const shape = useMemo(() => layoutGraph(graph), [shapeKey]); // eslint-disable-line react-hooks/exhaustive-deps
  const reduced = useReduced();
  const [zoom, setZoom] = useState(0.65);
  const [pan, setPan] = useState({ x: 20, y: 20 });
  const [fitsAtReadable, setFitsAtReadable] = useState(true);
  const viewport = useRef<HTMLDivElement | null>(null);
  const drag = useRef<{ x: number; y: number; px: number; py: number; moved: boolean } | null>(null);
  const near = selected ? neighbours(graph, selected) : null;

  const fit = useCallback(
    (floor: number) => {
      const box = viewport.current?.getBoundingClientRect();
      if (!box) return;
      const ideal = Math.min(1, (box.width - 32) / shape.width, (box.height - 32) / shape.height);
      setFitsAtReadable(ideal >= READABLE);
      const z = clamp(ideal, floor, MAX_ZOOM);
      if (ideal >= floor) {
        setPan({ x: (box.width - shape.width * z) / 2, y: (box.height - shape.height * z) / 2 });
      } else {
        // 放不下：停在能读的大小，镜头对准最左边一个还没完的任务，左边留一列它的前置
        const open = graph.nodes
          .filter((node) => node.tile.column !== 'done')
          .map((node) => shape.nodes[node.task.id])
          .sort((a, b) => a.x - b.x || a.y - b.y)[0];
        const target = open ?? Object.values(shape.nodes)[0];
        const left = Math.max(0, target.x - PAD - (NODE_W + COL_GAP));
        setPan({ x: 16 - left * z, y: box.height / 2 - (target.y + NODE_H / 2) * z });
      }
      setZoom(z);
    },
    [shape], // eslint-disable-line react-hooks/exhaustive-deps
  );

  // 换图或第一次打开：缩到放得下，不小于能读的大小（状态变了不动镜头）
  useLayoutEffect(() => fit(READABLE), [fit]);

  const zoomAt = (next: number, cx?: number, cy?: number) => {
    const box = viewport.current?.getBoundingClientRect();
    const z = clamp(next, MIN_ZOOM, MAX_ZOOM);
    const px = cx ?? (box ? box.width / 2 : 0);
    const py = cy ?? (box ? box.height / 2 : 0);
    setPan((p) => ({ x: px - ((px - p.x) / zoom) * z, y: py - ((py - p.y) / zoom) * z }));
    setZoom(z);
  };

  useEffect(() => {
    const el = viewport.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        const box = el.getBoundingClientRect();
        zoomAt(zoom * Math.exp(-e.deltaY * 0.0025), e.clientX - box.left, e.clientY - box.top);
      } else {
        setPan((p) => ({ x: p.x - e.deltaX, y: p.y - e.deltaY }));
      }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  });

  const compact = zoom < READABLE;
  const iconBtn = 'grid size-7 shrink-0 place-items-center rounded-btn text-ink-2 hover:bg-hover hover:text-ink';

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2 text-2">
        {header}
        <span className="ml-auto" />
        <button type="button" data-kind="graph-zoom-out" aria-label="缩小任务图" className={iconBtn} onClick={() => zoomAt(zoom - 0.15)}>
          <Minus className="size-3.5" />
        </button>
        <span className="brand-type w-10 shrink-0 text-center text-1 text-ink-2" data-kind="graph-zoom">{Math.round(zoom * 100)}%</span>
        <button type="button" data-kind="graph-zoom-in" aria-label="放大任务图" className={iconBtn} onClick={() => zoomAt(zoom + 0.15)}>
          <Plus className="size-3.5" />
        </button>
        <button type="button" data-kind="graph-fit" className="shrink-0 whitespace-nowrap rounded-btn px-2 py-1 text-ink-2 hover:bg-hover hover:text-ink" onClick={() => fit(MIN_ZOOM)}>
          适合窗口
        </button>
        <button type="button" aria-label="任务图向左平移" className={iconBtn} onClick={() => setPan((p) => ({ ...p, x: p.x + 160 }))}>
          <ArrowLeft className="size-3.5" />
        </button>
        <button type="button" aria-label="任务图向右平移" className={iconBtn} onClick={() => setPan((p) => ({ ...p, x: p.x - 160 }))}>
          <ArrowRight className="size-3.5" />
        </button>
      </div>
      <div
        ref={viewport}
        data-kind="graph-viewport"
        className="relative min-h-[280px] flex-1 cursor-grab overflow-hidden bg-pane outline-none active:cursor-grabbing"
        style={{ backgroundImage: 'radial-gradient(var(--line) 1px, transparent 1px)', backgroundSize: '18px 18px' }}
        tabIndex={0}
        role="group"
        aria-label="任务图；拖动背景平移，Ctrl 加滚轮缩放，也可使用方向键"
        onPointerDown={(e) => {
          if ((e.target as HTMLElement).closest('button')) return;
          drag.current = { x: e.clientX, y: e.clientY, px: pan.x, py: pan.y, moved: false };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d) return;
          if (Math.abs(e.clientX - d.x) + Math.abs(e.clientY - d.y) > 3) d.moved = true;
          setPan({ x: d.px + e.clientX - d.x, y: d.py + e.clientY - d.y });
        }}
        onPointerUp={() => {
          if (drag.current && !drag.current.moved) onSelect(null);
          drag.current = null;
        }}
        onKeyDown={(e) => {
          const step: Record<string, [number, number]> = { ArrowLeft: [60, 0], ArrowRight: [-60, 0], ArrowUp: [0, 60], ArrowDown: [0, -60] };
          if (step[e.key]) {
            e.preventDefault();
            setPan((p) => ({ x: p.x + step[e.key][0], y: p.y + step[e.key][1] }));
          }
        }}
      >
        <div data-kind="graph-stage" className="absolute left-0 top-0 origin-top-left" style={{ width: shape.width, height: shape.height, transform: `translate(${pan.x}px,${pan.y}px) scale(${zoom})` }}>
          <svg className="absolute left-0 top-0 overflow-visible" width={shape.width} height={shape.height} aria-hidden="true">
            {graph.edges.map((edge) =>
              shape.edges[edge.id] ? (
                <Wire key={edge.id} edge={edge} route={shape.edges[edge.id]} color={color} lit={Boolean(near?.edges.has(edge.id))} dim={Boolean(near && !near.edges.has(edge.id))} reduced={reduced} />
              ) : null,
            )}
          </svg>
          {graph.nodes.map((node) =>
            shape.nodes[node.task.id] ? (
              <Station
                key={node.task.id}
                node={node}
                at={shape.nodes[node.task.id]}
                compact={compact}
                selected={selected === node.task.id}
                dim={Boolean(near && !near.nodes.has(node.task.id))}
                onSelect={() => onSelect(selected === node.task.id ? null : node.task.id)}
              />
            ) : null,
          )}
          {compact
            ? null
            : graph.edges.map((edge) => (shape.edges[edge.id] ? <Payload key={edge.id} edge={edge} route={shape.edges[edge.id]} dim={Boolean(near && !near.edges.has(edge.id))} /> : null))}
        </div>
      </div>
      {!fitsAtReadable ? (
        <div className="shrink-0 border-t border-line px-3.5 py-1.5 text-right text-1 text-ink-3" data-kind="graph-many">
          共 {graph.nodes.length} 个，拖动看其余的；「适合窗口」看全貌
        </div>
      ) : null}
    </div>
  );
}

/* ───────── 页 ───────── */

function countsText(graph: TaskGraph): string {
  if (graph.closed) return '都关了';
  const { counts } = graph;
  return [counts.queue ? '排队 ' + String(counts.queue) : '', counts.doing ? '在做 ' + String(counts.doing) : '', counts.wait ? '等你 ' + String(counts.wait) : '', counts.done ? '完成 ' + String(counts.done) : '']
    .filter(Boolean)
    .join(' · ');
}

function Legend({ single }: { single: number }) {
  return (
    <p className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-t border-line px-3.5 py-2 text-1 text-ink-3" data-kind="graph-legend">
      <span className="inline-flex items-center gap-1"><span className="graph-legend-box" data-shape="read" />只读</span>
      <span className="inline-flex items-center gap-1"><span className="graph-legend-box" data-shape="write" />写</span>
      <span className="inline-flex items-center gap-1"><span className="graph-legend-box" data-shape="milestone" />里程碑</span>
      <span className="inline-flex items-center gap-1"><span className="graph-legend-box" data-shape="program" />程序门</span>
      <span className="mx-1 h-3 w-px bg-line-2" />
      <span className="inline-flex items-center gap-1">
        <svg width="22" height="6" aria-hidden><path d="M0 3 H22" stroke="var(--ink-2)" strokeWidth="1.5" /></svg>
        拿到了
      </span>
      <span className="inline-flex items-center gap-1">
        <svg width="22" height="6" aria-hidden><path d="M0 3 H22" stroke="var(--ink-2)" strokeWidth="1.5" strokeDasharray="5 4" /></svg>
        还在等
      </span>
      <span className="inline-flex items-center gap-1">
        <svg width="22" height="6" aria-hidden><path d="M0 3 H22" stroke="var(--ink-2)" strokeWidth="2.5" strokeDasharray="6 5" strokeLinecap="round" className="graph-dash-flow" /></svg>
        在流（往那边走）
      </span>
      <span className="inline-flex items-center gap-1">
        <span className="brand-type rounded-badge border border-line bg-content px-1 text-1 leading-4 text-ink-2">第 1 份</span>
        线上流的是哪一份
      </span>
      {single ? <span className="ml-auto" data-kind="graph-single-count">另有 {String(single)} 个单派的任务，没有前后关系，在看板里</span> : null}
    </p>
  );
}

export function GraphPane({
  selected,
  onSelect,
  onClose,
}: {
  selected: string | null;
  onSelect: (id: string) => void;
  onClose: () => void;
}) {
  const work = useWorkStore((state) => state.work);
  const error = useWorkStore((state) => state.error);
  const { graphs, single } = useMemo(() => deriveGraphs(work), [work]);
  const [graphId, setGraphId] = useState<string | null>(null);
  const selectedGraph = selected ? graphs.find((graph) => graph.nodes.some((node) => node.task.id === selected)) : undefined;
  const index = Math.max(0, graphs.findIndex((graph) => graph.id === graphId));
  const graph = selectedGraph ?? graphs[index];
  const owner = graph && selected && graph.nodes.some((node) => node.task.id === selected) ? selected : null;

  useEffect(() => {
    if (!owner) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [owner, onClose]);

  if (!work) {
    return (
      <div className="grid h-full place-items-center text-2 text-ink-3" data-kind="task-graph-pane">
        {error ?? '正在取工作页…'}
      </div>
    );
  }

  if (!graph) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-6 text-2 text-ink-3" data-kind="task-graph-pane">
        <div data-kind="task-graph-empty">
          <p>还没有任务图。Kind 把一件事拆成几步、你确认了，这里就有一张。</p>
          {single ? <p className="mt-2 text-1">另有 {String(single)} 个单派的任务，没有前后关系，在看板里。</p> : null}
        </div>
      </div>
    );
  }

  const currentIndex = Math.max(0, graphs.findIndex((item) => item.id === graph.id));
  const color = LINE_COLORS[currentIndex % LINE_COLORS.length];
  const header = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className="flex min-w-0 items-center gap-1.5 rounded-btn px-1.5 py-0.5 text-ink hover:bg-hover" data-kind="graph-pick">
          <span className="h-[3px] w-4 shrink-0 rounded-full" style={{ backgroundColor: color }} />
          <span className="max-w-[60%] shrink-0 truncate">{graph.title}</span>
          <span className="min-w-0 truncate text-1 text-ink-3">{String(graph.nodes.length)} 个任务 · {countsText(graph)}</span>
          <ChevronDown className="size-3.5 shrink-0 text-ink-3" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-96">
        <DropdownMenuRadioGroup
          value={graph.id}
          onValueChange={(id) => {
            onClose();
            setGraphId(id);
          }}
        >
          {graphs.map((item) => (
            <DropdownMenuRadioItem key={item.id} value={item.id}>
              <span className="truncate">{item.title}</span>
              <span className="ml-auto shrink-0 pl-2 text-1 text-ink-3">{String(item.nodes.length)} 个 · {countsText(item)}</span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-kind="task-graph-pane" data-graph-id={graph.id}>
      <div className="p-3 pb-0">
        <div className={cn('grid min-h-0 flex-1 overflow-hidden rounded-card border border-line', owner ? 'grid-cols-[minmax(0,1fr)_320px]' : 'grid-cols-1')} style={{ height: '100%' }}>
          <div className="flex min-h-0 min-w-0 flex-col" style={{ height: '100%' }}>
            <Canvas
              key={graph.id}
              graph={graph}
              color={color}
              selected={owner}
              onSelect={(id) => {
                if (id) onSelect(id);
                else onClose();
              }}
              header={header}
            />
            <Legend single={single} />
          </div>
          {owner ? <Detail graph={graph} id={owner} onClose={onClose} /> : null}
        </div>
      </div>
    </div>
  );
}
