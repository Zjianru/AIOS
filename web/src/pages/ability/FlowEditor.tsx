import { useEffect, useMemo, useRef, useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent } from 'react';
import { createPortal } from 'react-dom';
import { useReducedMotion } from 'motion/react';
import {
  Archive,
  ArchiveRestore,
  LoaderCircle,
  MoreHorizontal,
  Plus,
  Save,
  Trash2,
  Undo2,
  X,
} from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { listFlowPalette, listFlowTemplates, listFlows, listNewFlowDrafts } from '@/data/api';
import type { NewFlowTemplate } from '@/data/flows';
import type {
  Flow,
  FlowDod,
  FlowEdge,
  FlowInstance,
  FlowNode,
  FlowNodeKind,
  FlowPoint,
  PalettePart,
} from '@/data/flows';
import {
  applyAllFixes,
  applyOps,
  canConnect,
  clone,
  cloneFlow,
  connectNodes,
  countOps,
  createNode,
  insertNodeOnEdge,
  kindRespond,
  layoutFlow,
  nextId,
  partNodeKind,
  readPositions,
  summarizeOps,
  validateFlow,
  writePositions,
} from './flowGraph';
import type { FlowIssue, KindOp } from './flowGraph';
import { FlowCanvas } from './FlowCanvas';
import type { DropResult, FlowCanvasHandle, PaletteDragState } from './FlowCanvas';
import { FlowInspector } from './FlowInspector';
import { NewFlowDialog } from './FlowDialogs';
import type { NewFlowChoice } from './FlowDialogs';
import { FlowGlyph } from './flowGlyphs';
import { cn } from '@/lib/utils';
import { useAppStore } from '@/app/store';
import { go } from '@/app/navigate';

type Mode = 'flow' | 'instance';

interface Snapshot {
  name: string;
  nodes: FlowNode[];
  edges: FlowEdge[];
}

interface Proposal {
  by: string;
  said: string;
  ops: KindOp[];
  preview: Flow;
  summary: string;
  instanceId: string | null;
}

const actionBase =
  'inline-flex h-7 items-center gap-1 rounded-btn px-2.5 text-2 whitespace-nowrap outline-none transition-colors focus-visible:ring-1 focus-visible:ring-line-2 disabled:cursor-not-allowed disabled:opacity-45';
const secondaryButton = actionBase + ' border border-line-2 text-ink-2 hover:bg-hover hover:text-ink';
const primaryButton = actionBase + ' bg-ink font-semibold text-content hover:opacity-90';
const iconButton =
  'grid size-7 shrink-0 place-items-center rounded-btn border border-line-2 text-ink-2 outline-none transition-colors hover:bg-hover hover:text-ink focus-visible:ring-1 focus-visible:ring-line-2 disabled:cursor-not-allowed disabled:opacity-40';

const DOT: Record<FlowInstance['state'], string> = {
  run: 'bg-run',
  wait: 'bg-wait',
  done: 'bg-done',
};

function formatVersion(flow: Flow): string {
  return flow.draft ? '草稿' : 'v' + flow.version;
}

function inUseCount(flow: Flow): number {
  return flow.instances.filter((instance) => instance.state !== 'done').length;
}

function today(): string {
  const now = new Date();
  return String(now.getMonth() + 1).padStart(2, '0') + '-' + String(now.getDate()).padStart(2, '0');
}

function sameIds(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((id, index) => id === b[index]);
}

function activeInstanceOf(flow: Flow): FlowInstance | null {
  return flow.instances.find((instance) => instance.state !== 'done') ?? flow.instances[0] ?? null;
}

function LineChip({ flow, lineId }: { flow: Flow; lineId: string }) {
  const line = flow.lines.find((item) => item.id === lineId);
  if (!line) return null;
  return (
    <span
      className="inline-grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-bold leading-none text-white"
      style={{ background: line.color }}
      title={line.name}
    >
      {line.index}
    </span>
  );
}

function ProjectTag({ project }: { project: NonNullable<FlowInstance['project']> }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <span
        className="grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white"
        style={{ background: project.color }}
      >
        {project.code}
      </span>
      <span className="truncate">{project.name}</span>
    </span>
  );
}

function InstancesPanel(props: {
  flow: Flow;
  disabled: boolean;
  onUpgrade: (instanceId: string) => void;
  onWriteBack: (instanceId: string) => void;
}) {
  const { flow, disabled, onUpgrade, onWriteBack } = props;
  return (
    <section className="shrink-0 border-t border-line bg-content px-4 pb-2 pt-2">
      <div className="flex min-w-0 items-baseline gap-2">
        <h3 className="shrink-0 text-3 font-semibold text-ink">
          照这个流程跑的 <span className="brand-type text-ink-3">{flow.instances.length}</span>
        </h3>
        <span className="truncate text-1 text-ink-3">
          每个实例固定在开跑时的版本；流程改了不会动在跑的事，要升级就逐个点。
        </span>
      </div>
      {!flow.instances.length ? (
        <div className="pt-1.5 text-2 text-ink-3">还没有。保存后，新建任务时就能选它。</div>
      ) : (
        <div className="mt-1 max-h-[128px] overflow-y-auto">
          {flow.instances.map((instance) => {
            const live = instance.state !== 'done';
            const old = instance.version < flow.version;
            return (
              <div
                key={instance.id}
                className="grid grid-cols-[minmax(0,1.2fr)_minmax(0,.85fr)_minmax(0,1fr)_minmax(0,1.65fr)_minmax(0,1.25fr)] items-center gap-3 border-t border-line py-1.5 first:border-t-0"
              >
                <div className="min-w-0">
                  <b className="block truncate text-2 font-semibold text-ink">{instance.name}</b>
                  <span className="block truncate text-1 text-ink-3">
                    {instance.project ? <ProjectTag project={instance.project} /> : '未标项目'}
                  </span>
                </div>
                <div className="flex min-w-0 flex-wrap items-center gap-1">
                  <span className="rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2">
                    v{instance.version}
                  </span>
                  {old && live ? (
                    <span className="text-1 text-run">有新版 v{flow.version}</span>
                  ) : (
                    <span className="text-1 text-ink-3">固定</span>
                  )}
                </div>
                <div className="flex min-w-0 items-center gap-1.5 text-2 text-ink-2">
                  <i className={cn('size-[7px] shrink-0 rounded-full', DOT[instance.state])} />
                  <span className="truncate">{instance.stateText}</span>
                </div>
                <div className="min-w-0 text-1">
                  {instance.overrides.length ? (
                    <>
                      <span className="block text-ink">本地改了 {instance.overrides.length} 处</span>
                      <span className="block truncate text-ink-3">
                        {instance.overrides.map((override) => override.label).join('；')}
                      </span>
                    </>
                  ) : (
                    <span className="text-ink-3">照流程跑，没改</span>
                  )}
                </div>
                <div className="flex min-w-0 flex-wrap items-center justify-end gap-1">
                  {!live ? (
                    <span className="text-1 text-ink-3">已完成，留在 v{instance.version}</span>
                  ) : (
                    <>
                      {instance.overrides.length ? (
                        <button
                          type="button"
                          onClick={() => onWriteBack(instance.id)}
                          disabled={disabled}
                          className={cn(secondaryButton, '!h-6 !px-2 !text-1')}
                        >
                          写回流程
                        </button>
                      ) : null}
                      {old ? (
                        <button
                          type="button"
                          onClick={() => onUpgrade(instance.id)}
                          disabled={disabled}
                          className={cn(secondaryButton, '!h-6 !px-2 !text-1')}
                        >
                          升级到 v{flow.version}
                        </button>
                      ) : null}
                    </>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function IssuesPanel(props: {
  errors: FlowIssue[];
  infos: FlowIssue[];
  fixable: boolean;
  preview: boolean;
  onSelect: (ref: string) => void;
  onFix: (issue: FlowIssue) => void;
  onFixAll: () => void;
  onAck: (issue: FlowIssue) => void;
}) {
  const { errors, infos, fixable, preview, onSelect, onFix, onFixAll, onAck } = props;
  if (!errors.length && !infos.length) {
    return (
      <div className="flex h-8 shrink-0 items-center gap-1.5 border-t border-line px-4 text-2 text-ink-2">
        <i className="size-[7px] rounded-full bg-done" />
        {preview ? '检查都过了；接受后会存为新版本' : fixable ? '检查都过了，可以保存' : '检查都过了'}
      </div>
    );
  }
  return (
    <div className="max-h-[96px] shrink-0 overflow-y-auto border-t border-line px-4 py-1.5">
      <div className="flex items-baseline gap-2 text-2">
        {errors.length ? (
          <>
            <b className="font-semibold text-fail">要补的 {errors.length} 处</b>
            <span className="text-ink-3">补完才能保存</span>
          </>
        ) : (
          <b className="font-semibold text-done">检查都过了</b>
        )}
        {fixable && errors.length > 1 ? (
          <button type="button" onClick={onFixAll} className="ml-auto text-2 font-semibold text-run hover:underline">
            全部按建议补上
          </button>
        ) : null}
      </div>
      {errors.map((issue) => (
        <div key={issue.id} className="group flex items-center gap-2 rounded-btn px-0 py-0.5 hover:bg-hover">
          <i className="size-[7px] shrink-0 rounded-full bg-fail" />
          <button
            type="button"
            onClick={() => onSelect(issue.ref)}
            className="min-w-0 flex-1 truncate text-left text-2 text-ink-2 hover:text-ink"
            title={issue.message}
          >
            {issue.message}
          </button>
          {fixable ? (
            <button type="button" onClick={() => onFix(issue)} className="shrink-0 text-1 font-semibold text-run hover:underline">
              {issue.fixLabel}
            </button>
          ) : null}
        </div>
      ))}
      {infos.map((issue) => (
        <div key={issue.id} className="group flex items-center gap-2 py-0.5">
          <i className="size-[7px] shrink-0 rounded-full bg-wait" />
          <button
            type="button"
            onClick={() => onSelect(issue.ref)}
            className="min-w-0 flex-1 truncate text-left text-2 text-ink-2 hover:text-ink"
            title={issue.message}
          >
            {issue.message}
          </button>
          {fixable ? (
            <button type="button" onClick={() => onAck(issue)} className="shrink-0 text-1 font-semibold text-run hover:underline">
              知道了
            </button>
          ) : null}
        </div>
      ))}
    </div>
  );
}

/** 编辑器要用的引导数据：都从 src/data/api.ts 取，组件里不直接 import 样例数据。 */
type NewFlowDraftKey = 'blank' | 'say' | 'weekly' | 'writing' | 'podcast';

interface FlowBoot {
  flows: Flow[];
  palette: PalettePart[];
  templates: NewFlowTemplate[];
  drafts: Record<NewFlowDraftKey, Flow>;
}

/** 先把数据取回来，再挂编辑器本体（编辑器本体假定数据都在）。 */
export function FlowEditor({ flowId }: { flowId: string }): JSX.Element {
  const [boot, setBoot] = useState<FlowBoot | null>(null);
  const devStressFlow = useAppStore((state) => state.devStressFlow);
  useEffect(() => {
    let alive = true;
    void (async () => {
      const [flows, palette, templates, drafts] = await Promise.all([
        listFlows({ includeStress: devStressFlow }),
        listFlowPalette(),
        listFlowTemplates(),
        listNewFlowDrafts(),
      ]);
      if (!alive) return;
      setBoot({ flows, palette, templates, drafts });
    })();
    return () => {
      alive = false;
    };
  }, [devStressFlow]);
  if (!boot) {
    return <div className="grid h-full place-items-center text-3 text-ink-3">正在取流程…</div>;
  }
  return <FlowEditorBody flowId={flowId} boot={boot} />;
}

function FlowEditorBody({ flowId, boot }: { flowId: string; boot: FlowBoot }): JSX.Element {
  const systemReduced = useReducedMotion();
  const [flows, setFlows] = useState<Flow[]>(() =>
    boot.flows.map((seed) => {
      const flow = clone(seed);
      const stored = readPositions(flow.id);
      if (!stored) return flow;
      return {
        ...flow,
        nodes: flow.nodes.map((node) =>
          stored[node.id] ? { ...node, position: stored[node.id] } : node,
        ),
      };
    }),
  );
  const [currentId, setCurrentId] = useState(() =>
    boot.flows.some((flow) => flow.id === flowId) ? flowId : boot.flows[0].id,
  );
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [history, setHistory] = useState<Snapshot[]>([]);
  const [proposal, setProposal] = useState<Proposal | null>(null);
  const [mode, setMode] = useState<Mode>('flow');
  const [menuOpen, setMenuOpen] = useState(false);
  const [newOpen, setNewOpen] = useState(false);
  const [minimap, setMinimap] = useState(true);
  const [paletteDrag, setPaletteDrag] = useState<PaletteDragState | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [kindText, setKindText] = useState('');
  const [kindThinking, setKindThinking] = useState(false);
  const [kindMsg, setKindMsg] = useState<string | null>(null);
  const [layoutBusy, setLayoutBusy] = useState(false);
  const [layoutAnimating, setLayoutAnimating] = useState(false);
  const [fitToken, setFitToken] = useState(0);

  const current = flows.find((flow) => flow.id === currentId) ?? flows[0];
  const flowsRef = useRef(flows);
  const currentRef = useRef(current);
  const currentIdRef = useRef(currentId);
  flowsRef.current = flows;
  currentRef.current = current;
  currentIdRef.current = currentId;

  const canvasRef = useRef<FlowCanvasHandle | null>(null);
  const kindInputRef = useRef<HTMLInputElement | null>(null);
  const kindTimer = useRef<number | null>(null);
  const noticeTimer = useRef<number | null>(null);
  const layoutTimer = useRef<number | null>(null);
  const paletteStopRef = useRef<(() => void) | null>(null);
  const lastEditAt = useRef(0);

  useEffect(() => {
    if (!flows.some((flow) => flow.id === currentId) && flows[0]) {
      setCurrentId(flows[0].id);
    }
  }, [currentId, flows]);

  useEffect(() => {
    if (currentIdRef.current === flowId) return;
    const match = flowsRef.current.find((flow) => flow.id === flowId);
    if (!match) return;
    const outgoing = flowsRef.current.find((flow) => flow.id === currentIdRef.current);
    if (outgoing) writePositions(outgoing.id, outgoing.nodes);
    resetFlowView();
    setCurrentId(flowId);
    // resetFlowView 只重置视图状态，不依赖新的 flowId
  }, [flowId]);

  useEffect(
    () => () => {
      if (kindTimer.current) window.clearTimeout(kindTimer.current);
      if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
      if (layoutTimer.current) window.clearTimeout(layoutTimer.current);
      paletteStopRef.current?.();
    },
    [],
  );

  const baseValidation = useMemo(() => validateFlow(current), [current]);
  const activeInstance = activeInstanceOf(current);
  const instanceFlow = useMemo(() => {
    if (!activeInstance || !activeInstance.overrides.length) return current;
    const patches = new Map(activeInstance.overrides.map((override) => [override.nodeId, override.patch]));
    return {
      ...current,
      nodes: current.nodes.map((node) => {
        const patch = patches.get(node.id);
        return patch ? { ...node, ...clone(patch) } : node;
      }),
    };
  }, [activeInstance, current]);
  const displayFlow = proposal ? proposal.preview : mode === 'instance' ? instanceFlow : current;
  const validation = proposal ? validateFlow(proposal.preview) : baseValidation;
  const dirty = history.length > 0 || Boolean(current.draft);
  const selectedNode =
    selectedIds.length === 1
      ? current.nodes.find((node) => node.id === selectedIds[0]) ?? null
      : null;

  const suggestions = useMemo(() => {
    const out: string[] = [];
    const hasDouyin = current.nodes.some((node) => node.name === '抖音');
    const firstGate = current.nodes.find((node) => node.kind === 'gate');
    const hasRhythm = current.nodes.some((node) => Boolean(node.rhythm));
    if (hasDouyin && firstGate && hasRhythm) out.push('去掉抖音，评审前先查重，选题改到每天 9:00');
    const quality = current.nodes.find((node) => node.name === '质量门');
    if (quality && !(quality.members ?? []).includes('安全')) out.push('质量门也加上安全检查');
    if (current.nodes.some((node) => node.ext)) out.push('发布改成一天同意一次就行');
    if (
      selectedNode &&
      selectedNode.kind !== 'pool' &&
      selectedNode.kind !== 'start' &&
      selectedNode.kind !== 'end' &&
      selectedNode.kind !== 'join' &&
      !selectedNode.loop
    ) {
      out.push('「' + selectedNode.name + '」最多循环 3 次');
    }
    return out;
  }, [current, selectedNode]);

  function showNotice(text: string): void {
    setNotice(text);
    if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => {
      setNotice(null);
      noticeTimer.current = null;
    }, 2800);
  }

  function cancelKind(): void {
    if (kindTimer.current) {
      window.clearTimeout(kindTimer.current);
      kindTimer.current = null;
    }
    setKindThinking(false);
  }

  function resetFlowView(): void {
    cancelKind();
    setSelectedIds([]);
    setHistory([]);
    setProposal(null);
    setKindMsg(null);
    setKindText('');
    setMenuOpen(false);
    setLayoutBusy(false);
    setLayoutAnimating(false);
    setMode('flow');
    setFitToken((value) => value + 1);
  }

  function switchFlow(id: string): void {
    if (id === current.id) return;
    const target = flows.find((flow) => flow.id === id);
    writePositions(current.id, current.nodes);
    resetFlowView();
    setCurrentId(id);
    // 面包屑与地址栏跟着走；草稿没有稳定的可访问 id，不同步
    if (target && !target.draft) go('flow:' + id);
  }

  function pushUndo(): void {
    const snapshot: Snapshot = {
      name: current.name,
      nodes: clone(current.nodes),
      edges: clone(current.edges),
    };
    setHistory((prev) => [...prev.slice(-59), snapshot]);
  }

  function beginFieldEdit(): void {
    const now = Date.now();
    if (now - lastEditAt.current > 800) pushUndo();
    lastEditAt.current = now;
  }

  function patchFlowName(name: string): void {
    if (proposal || mode === 'instance' || current.archived) return;
    beginFieldEdit();
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? { ...flow, name } : flow)));
  }

  function patchNode(id: string, patch: Partial<FlowNode>): void {
    if (proposal || mode === 'instance' || current.archived) return;
    beginFieldEdit();
    setFlows((prev) =>
      prev.map((flow) =>
        flow.id === current.id
          ? { ...flow, nodes: flow.nodes.map((node) => (node.id === id ? { ...node, ...patch } : node)) }
          : flow,
      ),
    );
  }

  function updateCurrentNode(id: string, updater: (node: FlowNode) => FlowNode, undo: boolean): void {
    if (proposal || mode === 'instance' || current.archived) return;
    if (undo) pushUndo();
    setFlows((prev) =>
      prev.map((flow) =>
        flow.id === current.id
          ? { ...flow, nodes: flow.nodes.map((node) => (node.id === id ? updater(node) : node)) }
          : flow,
      ),
    );
  }

  function setNodeKind(id: string, kind: FlowNodeKind): void {
    updateCurrentNode(
      id,
      (node) => {
        const next: FlowNode = { ...node, kind };
        if (kind === 'gate') {
          next.members = next.members ?? [];
          next.rule = next.rule ?? '全部通过';
          next.fail = next.fail ?? '退回上一站改。';
        }
        if (kind === 'station' || kind === 'stack') next.who = next.who ?? [];
        if (kind === 'pool' && !next.note) next.note = '上游攒着，下游按自己的节奏来取。';
        return next;
      },
      true,
    );
  }

  function addToArray(id: string, field: 'who' | 'members' | 'skills', value: string): void {
    updateCurrentNode(id, (node) => ({ ...node, [field]: [...(node[field] ?? []), value] }), true);
  }

  function removeFromArray(id: string, field: 'who' | 'members' | 'skills', index: number): void {
    updateCurrentNode(
      id,
      (node) => ({ ...node, [field]: (node[field] ?? []).filter((_, at) => at !== index) }),
      true,
    );
  }

  function addDod(id: string): void {
    updateCurrentNode(id, (node) => ({ ...node, dod: [...(node.dod ?? []), { text: '', auto: true }] }), true);
  }

  function patchDod(id: string, index: number, patch: Partial<FlowDod>): void {
    if (proposal || mode === 'instance' || current.archived) return;
    beginFieldEdit();
    setFlows((prev) =>
      prev.map((flow) =>
        flow.id === current.id
          ? {
              ...flow,
              nodes: flow.nodes.map((node) =>
                node.id === id
                  ? { ...node, dod: (node.dod ?? []).map((item, at) => (at === index ? { ...item, ...patch } : item)) }
                  : node,
              ),
            }
          : flow,
      ),
    );
  }

  function removeDod(id: string, index: number): void {
    updateCurrentNode(
      id,
      (node) => ({ ...node, dod: (node.dod ?? []).filter((_, at) => at !== index) }),
      true,
    );
  }

  function deleteNodes(ids: string[]): void {
    if (proposal || mode === 'instance' || current.archived) return;
    const removable = new Set(
      ids.filter((id) => {
        const node = current.nodes.find((item) => item.id === id);
        return Boolean(node && node.kind !== 'start' && node.kind !== 'end');
      }),
    );
    if (!removable.size) {
      showNotice('起点和终点不能删');
      return;
    }
    pushUndo();
    setFlows((prev) =>
      prev.map((flow) =>
        flow.id === current.id
          ? {
              ...flow,
              nodes: flow.nodes.filter((node) => !removable.has(node.id)),
              edges: flow.edges.filter((edge) => !removable.has(edge.source) && !removable.has(edge.target)),
            }
          : flow,
      ),
    );
    setSelectedIds((prev) => prev.filter((id) => !removable.has(id)));
    showNotice('已删掉 ' + removable.size + ' 个站');
  }

  function onConnectNodes(source: string, target: string): void {
    if (proposal || mode === 'instance' || current.archived) return;
    const check = canConnect(current, source, target);
    if (!check.ok) {
      if (check.reason) showNotice(check.reason);
      return;
    }
    pushUndo();
    const next = connectNodes(current, source, target);
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
  }

  function dropPart(part: PalettePart['kind'], position: FlowPoint, edgeId: string | null): DropResult {
    if (proposal || mode === 'instance' || current.archived || layoutBusy) {
      return { ok: false, reason: mode === 'instance' ? '实例态只读，回到流程再改' : '现在不能改' };
    }
    const edge = edgeId ? current.edges.find((item) => item.id === edgeId) : null;
    const lineId = edge?.lineId ?? current.lines[0]?.id ?? 's1';
    const node = createNode(part, position, lineId);
    pushUndo();
    const next = edge ? insertNodeOnEdge(current, edge.id, node) : { ...current, nodes: [...current.nodes, node] };
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    setSelectedIds([node.id]);
    setFitToken((value) => value + 1);
    if (!edge) showNotice('放到空白处了，检查里会提示「没连上」');
    return { ok: true };
  }

  function undo(): void {
    if (!history.length || proposal || mode === 'instance' || current.archived) return;
    const snapshot = history[history.length - 1];
    setHistory((prev) => prev.slice(0, -1));
    setFlows((prev) =>
      prev.map((flow) =>
        flow.id === current.id
          ? { ...flow, name: snapshot.name, nodes: snapshot.nodes, edges: snapshot.edges }
          : flow,
      ),
    );
    setSelectedIds((prev) => prev.filter((id) => snapshot.nodes.some((node) => node.id === id)));
    writePositions(current.id, snapshot.nodes);
    showNotice('已撤销');
  }

  function applyIssueFix(issue: FlowIssue): void {
    if (proposal || mode === 'instance' || current.archived) return;
    pushUndo();
    const next = cloneFlow(current);
    issue.fix(next);
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    setSelectedIds((prev) => prev.filter((id) => next.nodes.some((node) => node.id === id)));
  }

  function fixAll(): void {
    if (proposal || mode === 'instance' || current.archived) return;
    const next = applyAllFixes(current);
    pushUndo();
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    setSelectedIds((prev) => prev.filter((id) => next.nodes.some((node) => node.id === id)));
    const left = validateFlow(next).errors.length;
    showNotice(left ? '能自动补的都补了，还有 ' + left + ' 处要手动' : '都按建议补上了，可以保存');
  }

  function acknowledge(issue: FlowIssue): void {
    if (proposal || mode === 'instance' || current.archived) return;
    pushUndo();
    const next = cloneFlow(current);
    issue.fix(next);
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
  }

  function saveVersion(): void {
    if (proposal || mode === 'instance' || current.archived || layoutBusy) return;
    const errors = validateFlow(current).errors.length;
    if (errors) {
      showNotice('还有 ' + errors + ' 处要补');
      return;
    }
    if (!dirty) return;
    const version = current.version + 1;
    const note = history.length ? '手动改了 ' + history.length + ' 处' : '第一次保存';
    const next: Flow = {
      ...current,
      version,
      draft: false,
      versions: [{ version, at: today(), note }, ...current.versions],
    };
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    setHistory([]);
    writePositions(current.id, current.nodes);
    const live = inUseCount(current);
    showNotice(
      live
        ? '已存为 v' + version + '。在跑的实例还在旧版本，要升级就在下面逐个点'
        : '已存为 v' + version,
    );
  }

  function archiveFlow(archived: boolean): void {
    if (proposal) return;
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? { ...flow, archived } : flow)));
    setMenuOpen(false);
    setSelectedIds([]);
    setHistory([]);
    setMode('flow');
    showNotice(archived ? '「' + current.name + '」已归档' : '「' + current.name + '」已恢复');
  }

  function deleteFlow(): void {
    if (proposal || inUseCount(current) > 0 || flows.length <= 1) return;
    const remaining = flows.filter((flow) => flow.id !== current.id);
    const nextCurrent = remaining[0];
    if (!nextCurrent) return;
    setFlows(remaining);
    setCurrentId(nextCurrent.id);
    resetFlowView();
    if (!nextCurrent.draft) go('flow:' + nextCurrent.id);
    showNotice('已删除「' + current.name + '」');
  }

  function upgradeInstance(instanceId: string): void {
    if (proposal || current.archived) return;
    const instance = current.instances.find((item) => item.id === instanceId);
    if (!instance) return;
    setFlows((prev) =>
      prev.map((flow) =>
        flow.id === current.id
          ? {
              ...flow,
              instances: flow.instances.map((item) =>
                item.id === instanceId ? { ...item, version: flow.version } : item,
              ),
            }
          : flow,
      ),
    );
    showNotice(
      instance.overrides.length
        ? '「' + instance.name + '」已升级到 v' + current.version + '，本地的 ' + instance.overrides.length + ' 处改动保留'
        : '「' + instance.name + '」已升级到 v' + current.version,
    );
  }

  function writeBackInstance(instanceId: string): void {
    if (proposal || current.archived) return;
    const instance = current.instances.find((item) => item.id === instanceId);
    if (!instance || !instance.overrides.length) return;
    const ops: KindOp[] = instance.overrides.map((override) => ({
      op: 'chg',
      ref: override.nodeId,
      patch: override.patch,
    }));
    const preview = applyOps(current, ops, true);
    setProposal({
      by: '写回实例「' + instance.name + '」的改动',
      said: instance.overrides.map((override) => override.label).join('；') + '。接受后这个实例也换到新版本。',
      ops,
      preview,
      summary: summarizeOps(current, ops),
      instanceId: instance.id,
    });
    setSelectedIds([]);
    setMode('flow');
    setKindMsg(null);
    setMenuOpen(false);
    setFitToken((value) => value + 1);
  }

  function acceptProposal(): void {
    if (!proposal) return;
    const accepted = applyOps(current, proposal.ops, false);
    const errors = validateFlow(accepted).errors.length;
    if (errors) {
      setKindMsg('接受之后还有 ' + errors + ' 处要补，先补完再接受。');
      return;
    }
    const version = current.version + 1;
    const note = proposal.summary + (proposal.instanceId ? '（写回实例改动）' : '');
    const next: Flow = {
      ...accepted,
      version,
      draft: false,
      versions: [{ version, at: today(), note }, ...current.versions],
      instances: current.instances.map((instance) =>
        instance.id === proposal.instanceId ? { ...instance, version, overrides: [] } : instance,
      ),
    };
    setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    setProposal(null);
    setSelectedIds([]);
    setHistory([]);
    setKindMsg(null);
    setMode('flow');
    writePositions(current.id, next.nodes);
    setFitToken((value) => value + 1);
    showNotice('已存为 v' + version + '：' + proposal.summary);
  }

  function rejectProposal(): void {
    setProposal(null);
    setSelectedIds([]);
    setKindMsg(null);
    setFitToken((value) => value + 1);
    showNotice('没有接受这次修改');
  }

  function submitKind(textArg?: string): void {
    const text = (textArg ?? kindText).trim();
    if (!text || kindThinking || proposal || mode === 'instance' || current.archived) return;
    cancelKind();
    setKindThinking(true);
    setKindMsg(null);
    const base = current;
    const selected = selectedIds[0] ?? null;
    kindTimer.current = window.setTimeout(() => {
      kindTimer.current = null;
      setKindThinking(false);
      if (currentRef.current !== base) return;
      const answer = kindRespond(base, text, selected);
      if (answer.reply) {
        setKindMsg(answer.reply);
        return;
      }
      if (!answer.ops?.length) {
        setKindMsg('我没对上具体哪一站。先在图上点一站再说，或者把站名写进来。');
        return;
      }
      const preview = applyOps(base, answer.ops, true);
      setProposal({
        by: 'Kind 的修改',
        said: '「' + text + '」',
        ops: answer.ops,
        preview,
        summary: summarizeOps(base, answer.ops),
        instanceId: null,
      });
      setSelectedIds([]);
      setKindText('');
      setKindMsg(null);
      setMode('flow');
      setFitToken((value) => value + 1);
    }, 420);
  }

  async function arrange(): Promise<void> {
    if (layoutBusy || proposal || mode === 'instance' || current.archived) return;
    pushUndo();
    setLayoutBusy(true);
    const reduce = Boolean(systemReduced) || document.documentElement.dataset.reducedMotion === 'true';
    if (!reduce) setLayoutAnimating(true);
    const target = current;
    try {
      const positions = await layoutFlow(target);
      const nextNodes = target.nodes.map((node) =>
        positions[node.id] ? { ...node, position: positions[node.id] } : node,
      );
      setFlows((prev) => prev.map((flow) => (flow.id === target.id ? { ...flow, nodes: nextNodes } : flow)));
      writePositions(target.id, nextNodes);
      setFitToken((value) => value + 1);
      if (layoutTimer.current) window.clearTimeout(layoutTimer.current);
      layoutTimer.current = window.setTimeout(() => {
        setLayoutAnimating(false);
        layoutTimer.current = null;
      }, 230);
    } catch {
      showNotice('排版没跑成，位置没变');
    } finally {
      setLayoutBusy(false);
    }
  }

  function createFlow(choice: NewFlowChoice): void {
    let base: Flow;
    if (choice === 'blank') base = clone(boot.drafts.blank);
    else if (choice === 'say') base = clone(boot.drafts.say);
    else if (choice === 'weekly') base = clone(boot.drafts.weekly);
    else if (choice === 'writing') base = clone(boot.drafts.writing);
    else if (choice === 'podcast') base = clone(boot.drafts.podcast);
    else if (choice === 'content') base = clone(boot.templates.find((item) => item.key === 'content')?.flow ?? boot.drafts.blank);
    else if (choice.startsWith('tpl-'))
      base = clone(boot.templates.find((item) => item.key === choice.slice(4))?.flow ?? boot.drafts.blank);
    else base = clone(boot.templates.find((item) => item.key === 'unfix')?.flow ?? boot.drafts.blank);
    const next: Flow = {
      ...base,
      id: nextId('flow'),
      version: 0,
      draft: true,
      archived: false,
      used: 0,
      versions: [],
      instances: [],
    };
    const errors = validateFlow(next).errors.length;
    setFlows((prev) => [...prev, next]);
    setNewOpen(false);
    setCurrentId(next.id);
    resetFlowView();
    showNotice(
      errors
        ? '草稿「' + next.name + '」：有 ' + errors + ' 处要补，补完才能保存'
        : '草稿「' + next.name + '」已就绪，检查都过了',
    );
  }

  function startPaletteDrag(part: PalettePart, event: ReactPointerEvent<HTMLButtonElement>): void {
    if (paletteDisabled || event.button !== 0) return;
    event.preventDefault();
    paletteStopRef.current?.();
    const initial: PaletteDragState = { part: part.kind, title: part.title, x: event.clientX, y: event.clientY };
    setPaletteDrag(initial);
    const move = (pointer: PointerEvent) => {
      setPaletteDrag({ part: part.kind, title: part.title, x: pointer.clientX, y: pointer.clientY });
    };
    const cleanup = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', cancel);
      paletteStopRef.current = null;
    };
    const cancel = () => {
      cleanup();
      setPaletteDrag(null);
    };
    const up = (pointer: PointerEvent) => {
      cleanup();
      setPaletteDrag(null);
      const result = canvasRef.current?.dropPart(part.kind, pointer.clientX, pointer.clientY);
      if (result && !result.ok && result.reason) showNotice(result.reason);
    };
    paletteStopRef.current = cancel;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', cancel);
  }

  function onCanvasSelection(ids: string[]): void {
    setSelectedIds((prev) => (sameIds(prev, ids) ? prev : ids));
  }

  function selectFromIssue(ref: string): void {
    if (displayFlow.nodes.some((node) => node.id === ref)) setSelectedIds([ref]);
  }

  function persistDragEnd(): void {
    window.setTimeout(() => {
      const flow = flowsRef.current.find((item) => item.id === currentIdRef.current);
      if (flow) writePositions(flow.id, flow.nodes);
    }, 30);
  }

  const preview = Boolean(proposal);
  const instanceMode = mode === 'instance';
  const paletteDisabled = preview || instanceMode || current.archived || layoutBusy;
  const kindDisabled = paletteDisabled;
  const saveable = !preview && !instanceMode && !current.archived;
  const errorCount = baseValidation.errors.length;
  const activeProject = activeInstance?.project;

  const keyHandlerRef = useRef<(event: KeyboardEvent) => void>(() => {});
  keyHandlerRef.current = (event: KeyboardEvent) => {
    const target = event.target as HTMLElement | null;
    const inField = Boolean(
      target &&
        (target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.tagName === 'SELECT' ||
          target.isContentEditable),
    );
    if (event.key === 'Escape') {
      if (newOpen) return;
      if (paletteDrag) {
        event.preventDefault();
        paletteStopRef.current?.();
        return;
      }
      if (menuOpen) {
        setMenuOpen(false);
        return;
      }
      if (selectedIds.length && !inField) setSelectedIds([]);
      return;
    }
    if (inField) return;
    if ((event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === 'z') {
      if (preview || instanceMode || current.archived) return;
      event.preventDefault();
      undo();
      return;
    }
    if (event.key === 'Delete' || event.key === 'Backspace') {
      if (!selectedIds.length || preview || instanceMode || current.archived) return;
      event.preventDefault();
      deleteNodes(selectedIds);
    }
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => keyHandlerRef.current(event);
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  return (
    <div className="relative flex h-full min-h-0 w-full flex-col overflow-hidden bg-content text-3 text-ink" data-flow-editor>
      <header className="shrink-0 px-4 pb-2 pt-2.5">
        <div className="flex items-center gap-2">
          <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto pb-0.5">
            {flows.map((flow) => (
              <button
                key={flow.id}
                type="button"
                onClick={() => switchFlow(flow.id)}
                aria-pressed={flow.id === current.id}
                className={cn(
                  'inline-flex h-6 shrink-0 items-center gap-1.5 rounded-btn px-2 text-2 transition-colors',
                  flow.id === current.id
                    ? 'bg-hover text-ink shadow-[inset_0_0_0_1px_var(--line-2)]'
                    : 'text-ink-3 hover:bg-hover hover:text-ink',
                  flow.archived && 'opacity-55',
                )}
              >
                <span className="max-w-[160px] truncate">{flow.name}</span>
                <span className="brand-type text-1 text-ink-3">{formatVersion(flow)}</span>
              </button>
            ))}
          </div>
          <div className="flex shrink-0 rounded-btn bg-hover p-0.5" role="group" aria-label="看流程还是看实例">
            <button
              type="button"
              onClick={() => {
                if (preview) return;
                setMode('flow');
                setSelectedIds([]);
              }}
              disabled={preview}
              aria-pressed={!instanceMode}
              className={cn(
                'h-6 rounded-[4px] px-2.5 text-1 transition-colors disabled:cursor-not-allowed',
                !instanceMode ? 'bg-ink font-semibold text-content' : 'text-ink-2 hover:text-ink',
              )}
            >
              流程
            </button>
            <button
              type="button"
              onClick={() => {
                if (preview) return;
                setMode('instance');
                setSelectedIds([]);
              }}
              disabled={preview}
              aria-pressed={instanceMode}
              className={cn(
                'h-6 rounded-[4px] px-2.5 text-1 transition-colors disabled:cursor-not-allowed',
                instanceMode ? 'bg-ink font-semibold text-content' : 'text-ink-2 hover:text-ink',
              )}
            >
              实例
            </button>
          </div>
        </div>

        <div className="mt-1 flex min-w-0 items-end gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-baseline gap-2">
              <span className="shrink-0 text-2 text-ink-3">流程</span>
              <h2 className="min-w-0 truncate text-5 font-semibold leading-6 text-ink" title={current.name}>
                {current.name}
              </h2>
              <span className="shrink-0 rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2">
                {formatVersion(current)}
              </span>
              {current.archived ? (
                <span className="shrink-0 rounded-badge border border-dashed border-line-2 px-1 text-1 text-ink-2">已归档</span>
              ) : null}
            </div>
            <div className="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5 text-2 text-ink-3">
              {instanceMode ? (
                activeInstance ? (
                  <>
                    <span className="text-ink-3">实例</span>
                    <span className="max-w-[220px] truncate text-ink-2">{activeInstance.name}</span>
                    {activeProject ? <ProjectTag project={activeProject} /> : null}
                    <span className="flex items-center gap-1">
                      {current.lines.map((line) => (
                        <LineChip key={line.id} flow={current} lineId={line.id} />
                      ))}
                    </span>
                    <span className="flex items-center gap-1">
                      <i className={cn('size-[7px] rounded-full', DOT[activeInstance.state])} />
                      {activeInstance.stateText}
                    </span>
                    <span>固定 v{activeInstance.version}</span>
                    {activeInstance.overrides.length ? (
                      <span className="text-ink-2">本地改了 {activeInstance.overrides.length} 处</span>
                    ) : null}
                  </>
                ) : (
                  <span>还没有实例。保存后，新建任务时就能选它。</span>
                )
              ) : (
                <>
                  <span className="max-w-[340px] truncate">{current.source}</span>
                  {current.used ? <span>用过 {current.used} 次</span> : null}
                  {current.instances.length ? <span>在用 {inUseCount(current)} 个实例</span> : null}
                  {history.length ? <span className="text-wait">未保存 {history.length} 处改动</span> : null}
                  {current.draft ? <span className="text-wait">草稿</span> : null}
                </>
              )}
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-1.5 pb-0.5">
            {saveable ? (
              <>
                <button
                  type="button"
                  onClick={undo}
                  disabled={!history.length}
                  className={iconButton}
                  title="撤销（⌘Z）"
                  aria-label="撤销"
                >
                  <Undo2 className="size-3.5" />
                </button>
                <button
                  type="button"
                  onClick={() => void arrange()}
                  disabled={layoutBusy}
                  className={cn(secondaryButton, '!px-2')}
                  title="从左到右重新分层"
                >
                  <LoaderCircle className={cn('size-3.5', layoutBusy && 'animate-spin')} />
                  整理
                </button>
                {dirty ? (
                  errorCount ? (
                    <button
                      type="button"
                      disabled
                      className={cn(actionBase, 'border border-fail/35 bg-fail-soft text-fail')}
                      title="补完才能保存"
                    >
                      还有 {errorCount} 处要补
                    </button>
                  ) : (
                    <button type="button" onClick={saveVersion} className={primaryButton}>
                      <Save className="size-3.5" />
                      保存为 v{current.version + 1}
                    </button>
                  )
                ) : (
                  <span className="px-1 text-2 text-ink-3">已保存</span>
                )}
              </>
            ) : instanceMode ? (
              <span className="rounded-badge border border-line-2 px-1.5 py-0.5 text-1 text-ink-3">实例只读</span>
            ) : preview ? (
              <span className="rounded-badge border border-run/40 bg-run/[0.06] px-1.5 py-0.5 text-1 text-run">看提议时只读</span>
            ) : null}
            <button type="button" onClick={() => setNewOpen(true)} disabled={preview} className={secondaryButton}>
              <Plus className="size-3.5" />
              新建流程
            </button>
            <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
              <DropdownMenuTrigger asChild>
                <button type="button" className={iconButton} disabled={preview} aria-label="更多" title="更多">
                  <MoreHorizontal className="size-4" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-[280px] bg-raised text-ink">
                <DropdownMenuItem onSelect={() => archiveFlow(!current.archived)} className="items-start gap-2 py-2">
                  {current.archived ? <ArchiveRestore className="mt-0.5 size-4 shrink-0" /> : <Archive className="mt-0.5 size-4 shrink-0" />}
                  <span className="min-w-0">
                    <span className="block text-2">{current.archived ? '恢复' : '归档'}</span>
                    <span className="block whitespace-normal text-1 leading-4 text-ink-3">
                      {current.archived
                        ? '重新出现在新建里。'
                        : '新建时不再出现；在跑的实例照常跑完，历史版本都留着。'}
                    </span>
                  </span>
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  disabled={inUseCount(current) > 0 || flows.length <= 1}
                  onSelect={deleteFlow}
                  className="items-start gap-2 py-2 text-fail data-[disabled]:text-ink-3"
                >
                  <Trash2 className="mt-0.5 size-4 shrink-0" />
                  <span className="min-w-0">
                    <span className="block text-2">删除</span>
                    <span className="block whitespace-normal text-1 leading-4 text-ink-3">
                      {inUseCount(current) > 0
                        ? '有 ' + inUseCount(current) + ' 个实例在用，在用的流程只能归档。'
                        : flows.length <= 1
                          ? '至少留一个流程。'
                          : '没有在用的实例，可以删。'}
                    </span>
                  </span>
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </header>

      <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_312px] overflow-hidden">
        <div className="flex min-h-0 flex-col">
          <div className="flex h-11 shrink-0 items-center gap-1 border-b border-line px-4">
            <span className="mr-1 shrink-0 text-1 font-semibold text-ink-3">零件</span>
            {boot.palette.map((part) => (
              <button
                key={part.kind}
                type="button"
                onPointerDown={(event) => startPaletteDrag(part, event)}
                disabled={paletteDisabled}
                title={part.description + '。拖到线上'}
                className="flex h-7 shrink-0 cursor-grab items-center gap-1 rounded-btn px-1.5 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-35"
              >
                <FlowGlyph kind={part.nodeKind} size={20} />
                <span>{part.title}</span>
              </button>
            ))}
            <div className="ml-auto flex min-w-0 items-center gap-2 overflow-hidden pl-2 text-1 text-ink-3">
              <span className="shrink-0">价值流</span>
              {current.lines.map((line) => (
                <span key={line.id} className="flex shrink-0 items-center gap-1">
                  <LineChip flow={current} lineId={line.id} />
                  <span className="max-w-[72px] truncate text-ink-2">{line.name}</span>
                </span>
              ))}
            </div>
          </div>

          <div className="relative shrink-0 px-4 pb-2 pt-1.5">
            {preview && proposal ? (
              <div className="flex min-h-[56px] flex-col justify-center gap-1">
                <div className="flex min-w-0 items-center gap-2">
                  <span className="grid size-6 shrink-0 place-items-center rounded-full bg-ink brand-type text-1 font-extrabold text-content">
                    {proposal.instanceId ? '实' : 'K'}
                  </span>
                  <b className="shrink-0 text-2 font-semibold text-ink">
                    {proposal.instanceId ? '写回实例的改动' : 'Kind 的修改'}
                  </b>
                  <span className="min-w-0 flex-1 truncate text-2 text-ink-2" title={proposal.said}>{proposal.said}</span>
                  <span className="flex shrink-0 items-center gap-2 brand-type text-2 font-semibold">
                    <span className="text-done">+{countOps(proposal.ops).add}<span className="ml-0.5 font-sans text-1 font-normal">新加</span></span>
                    <span className="text-fail">−{countOps(proposal.ops).del}<span className="ml-0.5 font-sans text-1 font-normal">去掉</span></span>
                    <span className="text-run">~{countOps(proposal.ops).chg}<span className="ml-0.5 font-sans text-1 font-normal">改了</span></span>
                  </span>
                  <button type="button" onClick={rejectProposal} className={secondaryButton}>不要</button>
                  <button type="button" onClick={acceptProposal} className={primaryButton}>
                    接受，存为 v{current.version + 1}
                  </button>
                </div>
                <div className="min-w-0 truncate pl-8 text-1 text-ink-3">
                  {proposal.instanceId ? proposal.by + ' · ' : ''}{proposal.summary} · 看提议时只读
                </div>
              </div>
            ) : instanceMode ? (
              <div className="flex min-h-[56px] items-center gap-2 text-2 text-ink-2">
                <span className="grid size-6 shrink-0 place-items-center rounded-full bg-hover text-1 font-semibold text-ink-2">实</span>
                <div className="min-w-0">
                  <div className="truncate">
                    <b className="font-semibold text-ink">实例态</b>
                    {activeInstance ? ' · 正在看 ' + activeInstance.name : ' · 这个流程还没有实例'}
                  </div>
                  <div className="truncate text-1 text-ink-3">图上的点表示这一轮走到哪；这里只读，回到「流程」再改。</div>
                </div>
                <button type="button" onClick={() => setMode('flow')} className={cn(secondaryButton, 'ml-auto shrink-0')}>
                  回到流程
                </button>
              </div>
            ) : (
              <>
                <form
                  className="flex items-center gap-2"
                  onSubmit={(event) => {
                    event.preventDefault();
                    submitKind();
                  }}
                >
                  <span className="grid size-6 shrink-0 place-items-center rounded-full bg-ink brand-type text-1 font-extrabold text-content">K</span>
                  {selectedNode ? (
                    <span className="shrink-0 rounded-badge bg-hover px-1.5 py-0.5 text-1 text-ink-2">指着「{selectedNode.name}」</span>
                  ) : null}
                  <input
                    ref={kindInputRef}
                    value={kindText}
                    onChange={(event) => setKindText(event.target.value)}
                    disabled={kindDisabled}
                    placeholder={
                      selectedNode
                        ? '跟 Kind 说这一站怎么改…'
                        : '跟 Kind 说怎么改；先点一站，就是指着它说'
                    }
                    aria-label="跟 Kind 说"
                    className="h-8 min-w-0 flex-1 rounded-btn border border-line-2 bg-raised px-2.5 text-2 text-ink outline-none placeholder:text-ink-3 focus:border-run disabled:cursor-not-allowed disabled:opacity-55"
                  />
                  <button type="submit" disabled={kindDisabled || kindThinking || !kindText.trim()} className={primaryButton}>
                    {kindThinking ? 'Kind 在看…' : '发给 Kind'}
                  </button>
                </form>
                <div className="mt-1 flex items-center gap-1.5 overflow-x-auto pl-8">
                  {suggestions.map((suggestion) => (
                    <button
                      key={suggestion}
                      type="button"
                      onClick={() => submitKind(suggestion)}
                      disabled={kindDisabled || kindThinking}
                      className="h-6 shrink-0 rounded-full bg-hover px-2.5 text-1 text-ink-2 transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-45"
                    >
                      {suggestion}
                    </button>
                  ))}
                  <span className="shrink-0 pl-1 text-1 text-ink-3">改动用绿加 / 红去 / 蓝改标出来</span>
                </div>
              </>
            )}
            {kindMsg ? (
              <div className="absolute left-4 right-4 top-full z-30 mt-1 flex items-start gap-2 rounded-card border border-line-2 bg-raised px-3 py-2 text-2 text-ink-2 shadow-[var(--shadow-pop)]">
                <span className="mt-0.5 grid size-5 shrink-0 place-items-center rounded-full bg-ink brand-type text-[9px] font-extrabold text-content">K</span>
                <span className="min-w-0 flex-1">{kindMsg}</span>
                <button type="button" onClick={() => setKindMsg(null)} className="grid size-5 shrink-0 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink" aria-label="关上 Kind 的回复">
                  <X className="size-3.5" />
                </button>
              </div>
            ) : null}
          </div>

          <div className="min-h-0 flex-1">
            <FlowCanvas
              ref={canvasRef}
              flow={displayFlow}
              instance={instanceMode ? activeInstance : null}
              selectedIds={selectedIds}
              readOnly={preview || instanceMode || Boolean(current.archived)}
              layoutAnimating={layoutAnimating}
              minimap={minimap}
              paletteDrag={paletteDrag}
              fitToken={fitToken}
              onMinimapChange={setMinimap}
              onSelectionChange={onCanvasSelection}
              onMoveNode={(id, position) => {
                if (preview || instanceMode || current.archived) return;
                setFlows((prev) =>
                  prev.map((flow) =>
                    flow.id === current.id
                      ? {
                          ...flow,
                          nodes: flow.nodes.map((node) => (node.id === id ? { ...node, position } : node)),
                        }
                      : flow,
                  ),
                );
              }}
              onMoveStart={() => {
                if (preview || instanceMode || current.archived) return;
                pushUndo();
              }}
              onMoveEnd={persistDragEnd}
              onConnectNodes={onConnectNodes}
              onRejectConnection={showNotice}
              onDropPart={dropPart}
            />
          </div>

          <IssuesPanel
            errors={validation.errors}
            infos={validation.infos}
            fixable={!preview && !instanceMode && !current.archived}
            preview={preview}
            onSelect={selectFromIssue}
            onFix={applyIssueFix}
            onFixAll={fixAll}
            onAck={acknowledge}
          />
        </div>

        <FlowInspector
          flow={displayFlow}
          selectedIds={selectedIds}
          preview={preview}
          readOnly={instanceMode || Boolean(current.archived)}
          archived={Boolean(current.archived)}
          instance={instanceMode ? activeInstance : null}
          onPatchFlowName={patchFlowName}
          onPointToKind={() => kindInputRef.current?.focus()}
          onClose={() => setSelectedIds([])}
          onSelect={(id) => setSelectedIds(id ? [id] : [])}
          onPatchNode={patchNode}
          onSetNodeKind={setNodeKind}
          onAddToArray={addToArray}
          onRemoveFromArray={removeFromArray}
          onAddDod={addDod}
          onPatchDod={patchDod}
          onRemoveDod={removeDod}
          onDeleteNode={(id) => deleteNodes([id])}
          onDeleteNodes={deleteNodes}
        />
      </div>

      <InstancesPanel
        flow={current}
        disabled={preview || Boolean(current.archived)}
        onUpgrade={upgradeInstance}
        onWriteBack={writeBackInstance}
      />

      <NewFlowDialog open={newOpen} onOpenChange={setNewOpen} onCreate={createFlow} templates={boot.templates} />

      {paletteDrag
        ? createPortal(
            <div
              className="pointer-events-none fixed z-[120] flex items-center gap-1.5 rounded-card border border-line-2 bg-raised px-2 py-1 text-2 font-semibold text-ink shadow-[var(--shadow-pop)]"
              style={{ left: paletteDrag.x + 14, top: paletteDrag.y + 12 }}
            >
              <FlowGlyph kind={partNodeKind(paletteDrag.part)} size={22} />
              {paletteDrag.title}
            </div>,
            document.body,
          )
        : null}

      {notice ? (
        <div
          role="status"
          className="pointer-events-none absolute bottom-3 left-1/2 z-40 max-w-[calc(100%-32px)] -translate-x-1/2 rounded-btn bg-ink px-3 py-1.5 text-2 text-content shadow-[var(--shadow-pop)]"
        >
          {notice}
        </div>
      ) : null}
    </div>
  );
}
