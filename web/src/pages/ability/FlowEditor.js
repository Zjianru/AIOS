import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useReducedMotion } from 'motion/react';
import { Archive, ArchiveRestore, LoaderCircle, MoreHorizontal, Plus, Save, Trash2, Undo2, X, } from 'lucide-react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, } from '@/components/ui/dropdown-menu';
import { listFlowPalette, listFlowTemplates, listFlows, listNewFlowDrafts } from '@/data/api';
import { applyAllFixes, applyOps, canConnect, clone, cloneFlow, connectNodes, countOps, createNode, insertNodeOnEdge, kindRespond, layoutFlow, nextId, partNodeKind, readPositions, summarizeOps, validateFlow, writePositions, } from './flowGraph';
import { FlowCanvas } from './FlowCanvas';
import { FlowInspector } from './FlowInspector';
import { NewFlowDialog } from './FlowDialogs';
import { FlowGlyph } from './flowGlyphs';
import { cn } from '@/lib/utils';
import { useAppStore } from '@/app/store';
import { go } from '@/app/navigate';
const actionBase = 'inline-flex h-7 items-center gap-1 rounded-btn px-2.5 text-2 whitespace-nowrap outline-none transition-colors focus-visible:ring-1 focus-visible:ring-line-2 disabled:cursor-not-allowed disabled:opacity-45';
const secondaryButton = actionBase + ' border border-line-2 text-ink-2 hover:bg-hover hover:text-ink';
const primaryButton = actionBase + ' bg-ink font-semibold text-content hover:opacity-90';
const iconButton = 'grid size-7 shrink-0 place-items-center rounded-btn border border-line-2 text-ink-2 outline-none transition-colors hover:bg-hover hover:text-ink focus-visible:ring-1 focus-visible:ring-line-2 disabled:cursor-not-allowed disabled:opacity-40';
const DOT = {
    run: 'bg-run',
    wait: 'bg-wait',
    done: 'bg-done',
};
function formatVersion(flow) {
    return flow.draft ? '草稿' : 'v' + flow.version;
}
function inUseCount(flow) {
    return flow.instances.filter((instance) => instance.state !== 'done').length;
}
function today() {
    const now = new Date();
    return String(now.getMonth() + 1).padStart(2, '0') + '-' + String(now.getDate()).padStart(2, '0');
}
function sameIds(a, b) {
    return a.length === b.length && a.every((id, index) => id === b[index]);
}
function activeInstanceOf(flow) {
    return flow.instances.find((instance) => instance.state !== 'done') ?? flow.instances[0] ?? null;
}
function LineChip({ flow, lineId }) {
    const line = flow.lines.find((item) => item.id === lineId);
    if (!line)
        return null;
    return (_jsx("span", { className: "inline-grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-bold leading-none text-white", style: { background: line.color }, title: line.name, children: line.index }));
}
function ProjectTag({ project }) {
    return (_jsxs("span", { className: "inline-flex min-w-0 items-center gap-1", children: [_jsx("span", { className: "grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: { background: project.color }, children: project.code }), _jsx("span", { className: "truncate", children: project.name })] }));
}
function InstancesPanel(props) {
    const { flow, disabled, onUpgrade, onWriteBack } = props;
    return (_jsxs("section", { className: "shrink-0 border-t border-line bg-content px-4 pb-2 pt-2", children: [_jsxs("div", { className: "flex min-w-0 items-baseline gap-2", children: [_jsxs("h3", { className: "shrink-0 text-3 font-semibold text-ink", children: ["\u7167\u8FD9\u4E2A\u6D41\u7A0B\u8DD1\u7684 ", _jsx("span", { className: "brand-type text-ink-3", children: flow.instances.length })] }), _jsx("span", { className: "truncate text-1 text-ink-3", children: "\u6BCF\u4E2A\u5B9E\u4F8B\u56FA\u5B9A\u5728\u5F00\u8DD1\u65F6\u7684\u7248\u672C\uFF1B\u6D41\u7A0B\u6539\u4E86\u4E0D\u4F1A\u52A8\u5728\u8DD1\u7684\u4E8B\uFF0C\u8981\u5347\u7EA7\u5C31\u9010\u4E2A\u70B9\u3002" })] }), !flow.instances.length ? (_jsx("div", { className: "pt-1.5 text-2 text-ink-3", children: "\u8FD8\u6CA1\u6709\u3002\u4FDD\u5B58\u540E\uFF0C\u65B0\u5EFA\u4EFB\u52A1\u65F6\u5C31\u80FD\u9009\u5B83\u3002" })) : (_jsx("div", { className: "mt-1 max-h-[128px] overflow-y-auto", children: flow.instances.map((instance) => {
                    const live = instance.state !== 'done';
                    const old = instance.version < flow.version;
                    return (_jsxs("div", { className: "grid grid-cols-[minmax(0,1.2fr)_minmax(0,.85fr)_minmax(0,1fr)_minmax(0,1.65fr)_minmax(0,1.25fr)] items-center gap-3 border-t border-line py-1.5 first:border-t-0", children: [_jsxs("div", { className: "min-w-0", children: [_jsx("b", { className: "block truncate text-2 font-semibold text-ink", children: instance.name }), _jsx("span", { className: "block truncate text-1 text-ink-3", children: instance.project ? _jsx(ProjectTag, { project: instance.project }) : '未标项目' })] }), _jsxs("div", { className: "flex min-w-0 flex-wrap items-center gap-1", children: [_jsxs("span", { className: "rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2", children: ["v", instance.version] }), old && live ? (_jsxs("span", { className: "text-1 text-run", children: ["\u6709\u65B0\u7248 v", flow.version] })) : (_jsx("span", { className: "text-1 text-ink-3", children: "\u56FA\u5B9A" }))] }), _jsxs("div", { className: "flex min-w-0 items-center gap-1.5 text-2 text-ink-2", children: [_jsx("i", { className: cn('size-[7px] shrink-0 rounded-full', DOT[instance.state]) }), _jsx("span", { className: "truncate", children: instance.stateText })] }), _jsx("div", { className: "min-w-0 text-1", children: instance.overrides.length ? (_jsxs(_Fragment, { children: [_jsxs("span", { className: "block text-ink", children: ["\u672C\u5730\u6539\u4E86 ", instance.overrides.length, " \u5904"] }), _jsx("span", { className: "block truncate text-ink-3", children: instance.overrides.map((override) => override.label).join('；') })] })) : (_jsx("span", { className: "text-ink-3", children: "\u7167\u6D41\u7A0B\u8DD1\uFF0C\u6CA1\u6539" })) }), _jsx("div", { className: "flex min-w-0 flex-wrap items-center justify-end gap-1", children: !live ? (_jsxs("span", { className: "text-1 text-ink-3", children: ["\u5DF2\u5B8C\u6210\uFF0C\u7559\u5728 v", instance.version] })) : (_jsxs(_Fragment, { children: [instance.overrides.length ? (_jsx("button", { type: "button", onClick: () => onWriteBack(instance.id), disabled: disabled, className: cn(secondaryButton, '!h-6 !px-2 !text-1'), children: "\u5199\u56DE\u6D41\u7A0B" })) : null, old ? (_jsxs("button", { type: "button", onClick: () => onUpgrade(instance.id), disabled: disabled, className: cn(secondaryButton, '!h-6 !px-2 !text-1'), children: ["\u5347\u7EA7\u5230 v", flow.version] })) : null] })) })] }, instance.id));
                }) }))] }));
}
function IssuesPanel(props) {
    const { errors, infos, fixable, preview, onSelect, onFix, onFixAll, onAck } = props;
    if (!errors.length && !infos.length) {
        return (_jsxs("div", { className: "flex h-8 shrink-0 items-center gap-1.5 border-t border-line px-4 text-2 text-ink-2", children: [_jsx("i", { className: "size-[7px] rounded-full bg-done" }), preview ? '检查都过了；接受后会存为新版本' : fixable ? '检查都过了，可以保存' : '检查都过了'] }));
    }
    return (_jsxs("div", { className: "max-h-[96px] shrink-0 overflow-y-auto border-t border-line px-4 py-1.5", children: [_jsxs("div", { className: "flex items-baseline gap-2 text-2", children: [errors.length ? (_jsxs(_Fragment, { children: [_jsxs("b", { className: "font-semibold text-fail", children: ["\u8981\u8865\u7684 ", errors.length, " \u5904"] }), _jsx("span", { className: "text-ink-3", children: "\u8865\u5B8C\u624D\u80FD\u4FDD\u5B58" })] })) : (_jsx("b", { className: "font-semibold text-done", children: "\u68C0\u67E5\u90FD\u8FC7\u4E86" })), fixable && errors.length > 1 ? (_jsx("button", { type: "button", onClick: onFixAll, className: "ml-auto text-2 font-semibold text-run hover:underline", children: "\u5168\u90E8\u6309\u5EFA\u8BAE\u8865\u4E0A" })) : null] }), errors.map((issue) => (_jsxs("div", { className: "group flex items-center gap-2 rounded-btn px-0 py-0.5 hover:bg-hover", children: [_jsx("i", { className: "size-[7px] shrink-0 rounded-full bg-fail" }), _jsx("button", { type: "button", onClick: () => onSelect(issue.ref), className: "min-w-0 flex-1 truncate text-left text-2 text-ink-2 hover:text-ink", title: issue.message, children: issue.message }), fixable ? (_jsx("button", { type: "button", onClick: () => onFix(issue), className: "shrink-0 text-1 font-semibold text-run hover:underline", children: issue.fixLabel })) : null] }, issue.id))), infos.map((issue) => (_jsxs("div", { className: "group flex items-center gap-2 py-0.5", children: [_jsx("i", { className: "size-[7px] shrink-0 rounded-full bg-wait" }), _jsx("button", { type: "button", onClick: () => onSelect(issue.ref), className: "min-w-0 flex-1 truncate text-left text-2 text-ink-2 hover:text-ink", title: issue.message, children: issue.message }), fixable ? (_jsx("button", { type: "button", onClick: () => onAck(issue), className: "shrink-0 text-1 font-semibold text-run hover:underline", children: "\u77E5\u9053\u4E86" })) : null] }, issue.id)))] }));
}
/** 先把数据取回来，再挂编辑器本体（编辑器本体假定数据都在）。 */
export function FlowEditor({ flowId }) {
    const [boot, setBoot] = useState(null);
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
            if (!alive)
                return;
            setBoot({ flows, palette, templates, drafts });
        })();
        return () => {
            alive = false;
        };
    }, [devStressFlow]);
    if (!boot) {
        return _jsx("div", { className: "grid h-full place-items-center text-3 text-ink-3", children: "\u6B63\u5728\u53D6\u6D41\u7A0B\u2026" });
    }
    return _jsx(FlowEditorBody, { flowId: flowId, boot: boot });
}
function FlowEditorBody({ flowId, boot }) {
    const systemReduced = useReducedMotion();
    const [flows, setFlows] = useState(() => boot.flows.map((seed) => {
        const flow = clone(seed);
        const stored = readPositions(flow.id);
        if (!stored)
            return flow;
        return {
            ...flow,
            nodes: flow.nodes.map((node) => stored[node.id] ? { ...node, position: stored[node.id] } : node),
        };
    }));
    const [currentId, setCurrentId] = useState(() => boot.flows.some((flow) => flow.id === flowId) ? flowId : boot.flows[0].id);
    const [selectedIds, setSelectedIds] = useState([]);
    const [history, setHistory] = useState([]);
    const [proposal, setProposal] = useState(null);
    const [mode, setMode] = useState('flow');
    const [menuOpen, setMenuOpen] = useState(false);
    const [newOpen, setNewOpen] = useState(false);
    const [minimap, setMinimap] = useState(true);
    const [paletteDrag, setPaletteDrag] = useState(null);
    const [notice, setNotice] = useState(null);
    const [kindText, setKindText] = useState('');
    const [kindThinking, setKindThinking] = useState(false);
    const [kindMsg, setKindMsg] = useState(null);
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
    const canvasRef = useRef(null);
    const kindInputRef = useRef(null);
    const kindTimer = useRef(null);
    const noticeTimer = useRef(null);
    const layoutTimer = useRef(null);
    const paletteStopRef = useRef(null);
    const lastEditAt = useRef(0);
    useEffect(() => {
        if (!flows.some((flow) => flow.id === currentId) && flows[0]) {
            setCurrentId(flows[0].id);
        }
    }, [currentId, flows]);
    useEffect(() => {
        if (currentIdRef.current === flowId)
            return;
        const match = flowsRef.current.find((flow) => flow.id === flowId);
        if (!match)
            return;
        const outgoing = flowsRef.current.find((flow) => flow.id === currentIdRef.current);
        if (outgoing)
            writePositions(outgoing.id, outgoing.nodes);
        resetFlowView();
        setCurrentId(flowId);
        // resetFlowView 只重置视图状态，不依赖新的 flowId
    }, [flowId]);
    useEffect(() => () => {
        if (kindTimer.current)
            window.clearTimeout(kindTimer.current);
        if (noticeTimer.current)
            window.clearTimeout(noticeTimer.current);
        if (layoutTimer.current)
            window.clearTimeout(layoutTimer.current);
        paletteStopRef.current?.();
    }, []);
    const baseValidation = useMemo(() => validateFlow(current), [current]);
    const activeInstance = activeInstanceOf(current);
    const instanceFlow = useMemo(() => {
        if (!activeInstance || !activeInstance.overrides.length)
            return current;
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
    const selectedNode = selectedIds.length === 1
        ? current.nodes.find((node) => node.id === selectedIds[0]) ?? null
        : null;
    const suggestions = useMemo(() => {
        const out = [];
        const hasDouyin = current.nodes.some((node) => node.name === '抖音');
        const firstGate = current.nodes.find((node) => node.kind === 'gate');
        const hasRhythm = current.nodes.some((node) => Boolean(node.rhythm));
        if (hasDouyin && firstGate && hasRhythm)
            out.push('去掉抖音，评审前先查重，选题改到每天 9:00');
        const quality = current.nodes.find((node) => node.name === '质量门');
        if (quality && !(quality.members ?? []).includes('安全'))
            out.push('质量门也加上安全检查');
        if (current.nodes.some((node) => node.ext))
            out.push('发布改成一天同意一次就行');
        if (selectedNode &&
            selectedNode.kind !== 'pool' &&
            selectedNode.kind !== 'start' &&
            selectedNode.kind !== 'end' &&
            selectedNode.kind !== 'join' &&
            !selectedNode.loop) {
            out.push('「' + selectedNode.name + '」最多循环 3 次');
        }
        return out;
    }, [current, selectedNode]);
    function showNotice(text) {
        setNotice(text);
        if (noticeTimer.current)
            window.clearTimeout(noticeTimer.current);
        noticeTimer.current = window.setTimeout(() => {
            setNotice(null);
            noticeTimer.current = null;
        }, 2800);
    }
    function cancelKind() {
        if (kindTimer.current) {
            window.clearTimeout(kindTimer.current);
            kindTimer.current = null;
        }
        setKindThinking(false);
    }
    function resetFlowView() {
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
    function switchFlow(id) {
        if (id === current.id)
            return;
        const target = flows.find((flow) => flow.id === id);
        writePositions(current.id, current.nodes);
        resetFlowView();
        setCurrentId(id);
        // 面包屑与地址栏跟着走；草稿没有稳定的可访问 id，不同步
        if (target && !target.draft)
            go('flow:' + id);
    }
    function pushUndo() {
        const snapshot = {
            name: current.name,
            nodes: clone(current.nodes),
            edges: clone(current.edges),
        };
        setHistory((prev) => [...prev.slice(-59), snapshot]);
    }
    function beginFieldEdit() {
        const now = Date.now();
        if (now - lastEditAt.current > 800)
            pushUndo();
        lastEditAt.current = now;
    }
    function patchFlowName(name) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        beginFieldEdit();
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? { ...flow, name } : flow)));
    }
    function patchNode(id, patch) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        beginFieldEdit();
        setFlows((prev) => prev.map((flow) => flow.id === current.id
            ? { ...flow, nodes: flow.nodes.map((node) => (node.id === id ? { ...node, ...patch } : node)) }
            : flow));
    }
    function updateCurrentNode(id, updater, undo) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        if (undo)
            pushUndo();
        setFlows((prev) => prev.map((flow) => flow.id === current.id
            ? { ...flow, nodes: flow.nodes.map((node) => (node.id === id ? updater(node) : node)) }
            : flow));
    }
    function setNodeKind(id, kind) {
        updateCurrentNode(id, (node) => {
            const next = { ...node, kind };
            if (kind === 'gate') {
                next.members = next.members ?? [];
                next.rule = next.rule ?? '全部通过';
                next.fail = next.fail ?? '退回上一站改。';
            }
            if (kind === 'station' || kind === 'stack')
                next.who = next.who ?? [];
            if (kind === 'pool' && !next.note)
                next.note = '上游攒着，下游按自己的节奏来取。';
            return next;
        }, true);
    }
    function addToArray(id, field, value) {
        updateCurrentNode(id, (node) => ({ ...node, [field]: [...(node[field] ?? []), value] }), true);
    }
    function removeFromArray(id, field, index) {
        updateCurrentNode(id, (node) => ({ ...node, [field]: (node[field] ?? []).filter((_, at) => at !== index) }), true);
    }
    function addDod(id) {
        updateCurrentNode(id, (node) => ({ ...node, dod: [...(node.dod ?? []), { text: '', auto: true }] }), true);
    }
    function patchDod(id, index, patch) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        beginFieldEdit();
        setFlows((prev) => prev.map((flow) => flow.id === current.id
            ? {
                ...flow,
                nodes: flow.nodes.map((node) => node.id === id
                    ? { ...node, dod: (node.dod ?? []).map((item, at) => (at === index ? { ...item, ...patch } : item)) }
                    : node),
            }
            : flow));
    }
    function removeDod(id, index) {
        updateCurrentNode(id, (node) => ({ ...node, dod: (node.dod ?? []).filter((_, at) => at !== index) }), true);
    }
    function deleteNodes(ids) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        const removable = new Set(ids.filter((id) => {
            const node = current.nodes.find((item) => item.id === id);
            return Boolean(node && node.kind !== 'start' && node.kind !== 'end');
        }));
        if (!removable.size) {
            showNotice('起点和终点不能删');
            return;
        }
        pushUndo();
        setFlows((prev) => prev.map((flow) => flow.id === current.id
            ? {
                ...flow,
                nodes: flow.nodes.filter((node) => !removable.has(node.id)),
                edges: flow.edges.filter((edge) => !removable.has(edge.source) && !removable.has(edge.target)),
            }
            : flow));
        setSelectedIds((prev) => prev.filter((id) => !removable.has(id)));
        showNotice('已删掉 ' + removable.size + ' 个站');
    }
    function onConnectNodes(source, target) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        const check = canConnect(current, source, target);
        if (!check.ok) {
            if (check.reason)
                showNotice(check.reason);
            return;
        }
        pushUndo();
        const next = connectNodes(current, source, target);
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    }
    function dropPart(part, position, edgeId) {
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
        if (!edge)
            showNotice('放到空白处了，检查里会提示「没连上」');
        return { ok: true };
    }
    function undo() {
        if (!history.length || proposal || mode === 'instance' || current.archived)
            return;
        const snapshot = history[history.length - 1];
        setHistory((prev) => prev.slice(0, -1));
        setFlows((prev) => prev.map((flow) => flow.id === current.id
            ? { ...flow, name: snapshot.name, nodes: snapshot.nodes, edges: snapshot.edges }
            : flow));
        setSelectedIds((prev) => prev.filter((id) => snapshot.nodes.some((node) => node.id === id)));
        writePositions(current.id, snapshot.nodes);
        showNotice('已撤销');
    }
    function applyIssueFix(issue) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        pushUndo();
        const next = cloneFlow(current);
        issue.fix(next);
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
        setSelectedIds((prev) => prev.filter((id) => next.nodes.some((node) => node.id === id)));
    }
    function fixAll() {
        if (proposal || mode === 'instance' || current.archived)
            return;
        const next = applyAllFixes(current);
        pushUndo();
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
        setSelectedIds((prev) => prev.filter((id) => next.nodes.some((node) => node.id === id)));
        const left = validateFlow(next).errors.length;
        showNotice(left ? '能自动补的都补了，还有 ' + left + ' 处要手动' : '都按建议补上了，可以保存');
    }
    function acknowledge(issue) {
        if (proposal || mode === 'instance' || current.archived)
            return;
        pushUndo();
        const next = cloneFlow(current);
        issue.fix(next);
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
    }
    function saveVersion() {
        if (proposal || mode === 'instance' || current.archived || layoutBusy)
            return;
        const errors = validateFlow(current).errors.length;
        if (errors) {
            showNotice('还有 ' + errors + ' 处要补');
            return;
        }
        if (!dirty)
            return;
        const version = current.version + 1;
        const note = history.length ? '手动改了 ' + history.length + ' 处' : '第一次保存';
        const next = {
            ...current,
            version,
            draft: false,
            versions: [{ version, at: today(), note }, ...current.versions],
        };
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? next : flow)));
        setHistory([]);
        writePositions(current.id, current.nodes);
        const live = inUseCount(current);
        showNotice(live
            ? '已存为 v' + version + '。在跑的实例还在旧版本，要升级就在下面逐个点'
            : '已存为 v' + version);
    }
    function archiveFlow(archived) {
        if (proposal)
            return;
        setFlows((prev) => prev.map((flow) => (flow.id === current.id ? { ...flow, archived } : flow)));
        setMenuOpen(false);
        setSelectedIds([]);
        setHistory([]);
        setMode('flow');
        showNotice(archived ? '「' + current.name + '」已归档' : '「' + current.name + '」已恢复');
    }
    function deleteFlow() {
        if (proposal || inUseCount(current) > 0 || flows.length <= 1)
            return;
        const remaining = flows.filter((flow) => flow.id !== current.id);
        const nextCurrent = remaining[0];
        if (!nextCurrent)
            return;
        setFlows(remaining);
        setCurrentId(nextCurrent.id);
        resetFlowView();
        if (!nextCurrent.draft)
            go('flow:' + nextCurrent.id);
        showNotice('已删除「' + current.name + '」');
    }
    function upgradeInstance(instanceId) {
        if (proposal || current.archived)
            return;
        const instance = current.instances.find((item) => item.id === instanceId);
        if (!instance)
            return;
        setFlows((prev) => prev.map((flow) => flow.id === current.id
            ? {
                ...flow,
                instances: flow.instances.map((item) => item.id === instanceId ? { ...item, version: flow.version } : item),
            }
            : flow));
        showNotice(instance.overrides.length
            ? '「' + instance.name + '」已升级到 v' + current.version + '，本地的 ' + instance.overrides.length + ' 处改动保留'
            : '「' + instance.name + '」已升级到 v' + current.version);
    }
    function writeBackInstance(instanceId) {
        if (proposal || current.archived)
            return;
        const instance = current.instances.find((item) => item.id === instanceId);
        if (!instance || !instance.overrides.length)
            return;
        const ops = instance.overrides.map((override) => ({
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
    function acceptProposal() {
        if (!proposal)
            return;
        const accepted = applyOps(current, proposal.ops, false);
        const errors = validateFlow(accepted).errors.length;
        if (errors) {
            setKindMsg('接受之后还有 ' + errors + ' 处要补，先补完再接受。');
            return;
        }
        const version = current.version + 1;
        const note = proposal.summary + (proposal.instanceId ? '（写回实例改动）' : '');
        const next = {
            ...accepted,
            version,
            draft: false,
            versions: [{ version, at: today(), note }, ...current.versions],
            instances: current.instances.map((instance) => instance.id === proposal.instanceId ? { ...instance, version, overrides: [] } : instance),
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
    function rejectProposal() {
        setProposal(null);
        setSelectedIds([]);
        setKindMsg(null);
        setFitToken((value) => value + 1);
        showNotice('没有接受这次修改');
    }
    function submitKind(textArg) {
        const text = (textArg ?? kindText).trim();
        if (!text || kindThinking || proposal || mode === 'instance' || current.archived)
            return;
        cancelKind();
        setKindThinking(true);
        setKindMsg(null);
        const base = current;
        const selected = selectedIds[0] ?? null;
        kindTimer.current = window.setTimeout(() => {
            kindTimer.current = null;
            setKindThinking(false);
            if (currentRef.current !== base)
                return;
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
    async function arrange() {
        if (layoutBusy || proposal || mode === 'instance' || current.archived)
            return;
        pushUndo();
        setLayoutBusy(true);
        const reduce = Boolean(systemReduced) || document.documentElement.dataset.reducedMotion === 'true';
        if (!reduce)
            setLayoutAnimating(true);
        const target = current;
        try {
            const positions = await layoutFlow(target);
            const nextNodes = target.nodes.map((node) => positions[node.id] ? { ...node, position: positions[node.id] } : node);
            setFlows((prev) => prev.map((flow) => (flow.id === target.id ? { ...flow, nodes: nextNodes } : flow)));
            writePositions(target.id, nextNodes);
            setFitToken((value) => value + 1);
            if (layoutTimer.current)
                window.clearTimeout(layoutTimer.current);
            layoutTimer.current = window.setTimeout(() => {
                setLayoutAnimating(false);
                layoutTimer.current = null;
            }, 230);
        }
        catch {
            showNotice('排版没跑成，位置没变');
        }
        finally {
            setLayoutBusy(false);
        }
    }
    function createFlow(choice) {
        let base;
        if (choice === 'blank')
            base = clone(boot.drafts.blank);
        else if (choice === 'say')
            base = clone(boot.drafts.say);
        else if (choice === 'weekly')
            base = clone(boot.drafts.weekly);
        else if (choice === 'writing')
            base = clone(boot.drafts.writing);
        else if (choice === 'podcast')
            base = clone(boot.drafts.podcast);
        else if (choice === 'content')
            base = clone(boot.templates.find((item) => item.key === 'content')?.flow ?? boot.drafts.blank);
        else if (choice.startsWith('tpl-'))
            base = clone(boot.templates.find((item) => item.key === choice.slice(4))?.flow ?? boot.drafts.blank);
        else
            base = clone(boot.templates.find((item) => item.key === 'unfix')?.flow ?? boot.drafts.blank);
        const next = {
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
        showNotice(errors
            ? '草稿「' + next.name + '」：有 ' + errors + ' 处要补，补完才能保存'
            : '草稿「' + next.name + '」已就绪，检查都过了');
    }
    function startPaletteDrag(part, event) {
        if (paletteDisabled || event.button !== 0)
            return;
        event.preventDefault();
        paletteStopRef.current?.();
        const initial = { part: part.kind, title: part.title, x: event.clientX, y: event.clientY };
        setPaletteDrag(initial);
        const move = (pointer) => {
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
        const up = (pointer) => {
            cleanup();
            setPaletteDrag(null);
            const result = canvasRef.current?.dropPart(part.kind, pointer.clientX, pointer.clientY);
            if (result && !result.ok && result.reason)
                showNotice(result.reason);
        };
        paletteStopRef.current = cancel;
        window.addEventListener('pointermove', move);
        window.addEventListener('pointerup', up);
        window.addEventListener('pointercancel', cancel);
    }
    function onCanvasSelection(ids) {
        setSelectedIds((prev) => (sameIds(prev, ids) ? prev : ids));
    }
    function selectFromIssue(ref) {
        if (displayFlow.nodes.some((node) => node.id === ref))
            setSelectedIds([ref]);
    }
    function persistDragEnd() {
        window.setTimeout(() => {
            const flow = flowsRef.current.find((item) => item.id === currentIdRef.current);
            if (flow)
                writePositions(flow.id, flow.nodes);
        }, 30);
    }
    const preview = Boolean(proposal);
    const instanceMode = mode === 'instance';
    const paletteDisabled = preview || instanceMode || current.archived || layoutBusy;
    const kindDisabled = paletteDisabled;
    const saveable = !preview && !instanceMode && !current.archived;
    const errorCount = baseValidation.errors.length;
    const activeProject = activeInstance?.project;
    const keyHandlerRef = useRef(() => { });
    keyHandlerRef.current = (event) => {
        const target = event.target;
        const inField = Boolean(target &&
            (target.tagName === 'INPUT' ||
                target.tagName === 'TEXTAREA' ||
                target.tagName === 'SELECT' ||
                target.isContentEditable));
        if (event.key === 'Escape') {
            if (newOpen)
                return;
            if (paletteDrag) {
                event.preventDefault();
                paletteStopRef.current?.();
                return;
            }
            if (menuOpen) {
                setMenuOpen(false);
                return;
            }
            if (selectedIds.length && !inField)
                setSelectedIds([]);
            return;
        }
        if (inField)
            return;
        if ((event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === 'z') {
            if (preview || instanceMode || current.archived)
                return;
            event.preventDefault();
            undo();
            return;
        }
        if (event.key === 'Delete' || event.key === 'Backspace') {
            if (!selectedIds.length || preview || instanceMode || current.archived)
                return;
            event.preventDefault();
            deleteNodes(selectedIds);
        }
    };
    useEffect(() => {
        const onKeyDown = (event) => keyHandlerRef.current(event);
        window.addEventListener('keydown', onKeyDown);
        return () => window.removeEventListener('keydown', onKeyDown);
    }, []);
    return (_jsxs("div", { className: "relative flex h-full min-h-0 w-full flex-col overflow-hidden bg-content text-3 text-ink", "data-flow-editor": true, children: [_jsxs("header", { className: "shrink-0 px-4 pb-2 pt-2.5", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsx("div", { className: "flex min-w-0 flex-1 items-center gap-1 overflow-x-auto pb-0.5", children: flows.map((flow) => (_jsxs("button", { type: "button", onClick: () => switchFlow(flow.id), "aria-pressed": flow.id === current.id, className: cn('inline-flex h-6 shrink-0 items-center gap-1.5 rounded-btn px-2 text-2 transition-colors', flow.id === current.id
                                        ? 'bg-hover text-ink shadow-[inset_0_0_0_1px_var(--line-2)]'
                                        : 'text-ink-3 hover:bg-hover hover:text-ink', flow.archived && 'opacity-55'), children: [_jsx("span", { className: "max-w-[160px] truncate", children: flow.name }), _jsx("span", { className: "brand-type text-1 text-ink-3", children: formatVersion(flow) })] }, flow.id))) }), _jsxs("div", { className: "flex shrink-0 rounded-btn bg-hover p-0.5", role: "group", "aria-label": "\u770B\u6D41\u7A0B\u8FD8\u662F\u770B\u5B9E\u4F8B", children: [_jsx("button", { type: "button", onClick: () => {
                                            if (preview)
                                                return;
                                            setMode('flow');
                                            setSelectedIds([]);
                                        }, disabled: preview, "aria-pressed": !instanceMode, className: cn('h-6 rounded-[4px] px-2.5 text-1 transition-colors disabled:cursor-not-allowed', !instanceMode ? 'bg-ink font-semibold text-content' : 'text-ink-2 hover:text-ink'), children: "\u6D41\u7A0B" }), _jsx("button", { type: "button", onClick: () => {
                                            if (preview)
                                                return;
                                            setMode('instance');
                                            setSelectedIds([]);
                                        }, disabled: preview, "aria-pressed": instanceMode, className: cn('h-6 rounded-[4px] px-2.5 text-1 transition-colors disabled:cursor-not-allowed', instanceMode ? 'bg-ink font-semibold text-content' : 'text-ink-2 hover:text-ink'), children: "\u5B9E\u4F8B" })] })] }), _jsxs("div", { className: "mt-1 flex min-w-0 items-end gap-3", children: [_jsxs("div", { className: "min-w-0 flex-1", children: [_jsxs("div", { className: "flex min-w-0 items-baseline gap-2", children: [_jsx("span", { className: "shrink-0 text-2 text-ink-3", children: "\u6D41\u7A0B" }), _jsx("h2", { className: "min-w-0 truncate text-5 font-semibold leading-6 text-ink", title: current.name, children: current.name }), _jsx("span", { className: "shrink-0 rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2", children: formatVersion(current) }), current.archived ? (_jsx("span", { className: "shrink-0 rounded-badge border border-dashed border-line-2 px-1 text-1 text-ink-2", children: "\u5DF2\u5F52\u6863" })) : null] }), _jsx("div", { className: "mt-0.5 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5 text-2 text-ink-3", children: instanceMode ? (activeInstance ? (_jsxs(_Fragment, { children: [_jsx("span", { className: "text-ink-3", children: "\u5B9E\u4F8B" }), _jsx("span", { className: "max-w-[220px] truncate text-ink-2", children: activeInstance.name }), activeProject ? _jsx(ProjectTag, { project: activeProject }) : null, _jsx("span", { className: "flex items-center gap-1", children: current.lines.map((line) => (_jsx(LineChip, { flow: current, lineId: line.id }, line.id))) }), _jsxs("span", { className: "flex items-center gap-1", children: [_jsx("i", { className: cn('size-[7px] rounded-full', DOT[activeInstance.state]) }), activeInstance.stateText] }), _jsxs("span", { children: ["\u56FA\u5B9A v", activeInstance.version] }), activeInstance.overrides.length ? (_jsxs("span", { className: "text-ink-2", children: ["\u672C\u5730\u6539\u4E86 ", activeInstance.overrides.length, " \u5904"] })) : null] })) : (_jsx("span", { children: "\u8FD8\u6CA1\u6709\u5B9E\u4F8B\u3002\u4FDD\u5B58\u540E\uFF0C\u65B0\u5EFA\u4EFB\u52A1\u65F6\u5C31\u80FD\u9009\u5B83\u3002" }))) : (_jsxs(_Fragment, { children: [_jsx("span", { className: "max-w-[340px] truncate", children: current.source }), current.used ? _jsxs("span", { children: ["\u7528\u8FC7 ", current.used, " \u6B21"] }) : null, current.instances.length ? _jsxs("span", { children: ["\u5728\u7528 ", inUseCount(current), " \u4E2A\u5B9E\u4F8B"] }) : null, history.length ? _jsxs("span", { className: "text-wait", children: ["\u672A\u4FDD\u5B58 ", history.length, " \u5904\u6539\u52A8"] }) : null, current.draft ? _jsx("span", { className: "text-wait", children: "\u8349\u7A3F" }) : null] })) })] }), _jsxs("div", { className: "flex shrink-0 items-center gap-1.5 pb-0.5", children: [saveable ? (_jsxs(_Fragment, { children: [_jsx("button", { type: "button", onClick: undo, disabled: !history.length, className: iconButton, title: "\u64A4\u9500\uFF08\u2318Z\uFF09", "aria-label": "\u64A4\u9500", children: _jsx(Undo2, { className: "size-3.5" }) }), _jsxs("button", { type: "button", onClick: () => void arrange(), disabled: layoutBusy, className: cn(secondaryButton, '!px-2'), title: "\u4ECE\u5DE6\u5230\u53F3\u91CD\u65B0\u5206\u5C42", children: [_jsx(LoaderCircle, { className: cn('size-3.5', layoutBusy && 'animate-spin') }), "\u6574\u7406"] }), dirty ? (errorCount ? (_jsxs("button", { type: "button", disabled: true, className: cn(actionBase, 'border border-fail/35 bg-fail-soft text-fail'), title: "\u8865\u5B8C\u624D\u80FD\u4FDD\u5B58", children: ["\u8FD8\u6709 ", errorCount, " \u5904\u8981\u8865"] })) : (_jsxs("button", { type: "button", onClick: saveVersion, className: primaryButton, children: [_jsx(Save, { className: "size-3.5" }), "\u4FDD\u5B58\u4E3A v", current.version + 1] }))) : (_jsx("span", { className: "px-1 text-2 text-ink-3", children: "\u5DF2\u4FDD\u5B58" }))] })) : instanceMode ? (_jsx("span", { className: "rounded-badge border border-line-2 px-1.5 py-0.5 text-1 text-ink-3", children: "\u5B9E\u4F8B\u53EA\u8BFB" })) : preview ? (_jsx("span", { className: "rounded-badge border border-run/40 bg-run/[0.06] px-1.5 py-0.5 text-1 text-run", children: "\u770B\u63D0\u8BAE\u65F6\u53EA\u8BFB" })) : null, _jsxs("button", { type: "button", onClick: () => setNewOpen(true), disabled: preview, className: secondaryButton, children: [_jsx(Plus, { className: "size-3.5" }), "\u65B0\u5EFA\u6D41\u7A0B"] }), _jsxs(DropdownMenu, { open: menuOpen, onOpenChange: setMenuOpen, children: [_jsx(DropdownMenuTrigger, { asChild: true, children: _jsx("button", { type: "button", className: iconButton, disabled: preview, "aria-label": "\u66F4\u591A", title: "\u66F4\u591A", children: _jsx(MoreHorizontal, { className: "size-4" }) }) }), _jsxs(DropdownMenuContent, { align: "end", className: "w-[280px] bg-raised text-ink", children: [_jsxs(DropdownMenuItem, { onSelect: () => archiveFlow(!current.archived), className: "items-start gap-2 py-2", children: [current.archived ? _jsx(ArchiveRestore, { className: "mt-0.5 size-4 shrink-0" }) : _jsx(Archive, { className: "mt-0.5 size-4 shrink-0" }), _jsxs("span", { className: "min-w-0", children: [_jsx("span", { className: "block text-2", children: current.archived ? '恢复' : '归档' }), _jsx("span", { className: "block whitespace-normal text-1 leading-4 text-ink-3", children: current.archived
                                                                            ? '重新出现在新建里。'
                                                                            : '新建时不再出现；在跑的实例照常跑完，历史版本都留着。' })] })] }), _jsx(DropdownMenuSeparator, {}), _jsxs(DropdownMenuItem, { disabled: inUseCount(current) > 0 || flows.length <= 1, onSelect: deleteFlow, className: "items-start gap-2 py-2 text-fail data-[disabled]:text-ink-3", children: [_jsx(Trash2, { className: "mt-0.5 size-4 shrink-0" }), _jsxs("span", { className: "min-w-0", children: [_jsx("span", { className: "block text-2", children: "\u5220\u9664" }), _jsx("span", { className: "block whitespace-normal text-1 leading-4 text-ink-3", children: inUseCount(current) > 0
                                                                            ? '有 ' + inUseCount(current) + ' 个实例在用，在用的流程只能归档。'
                                                                            : flows.length <= 1
                                                                                ? '至少留一个流程。'
                                                                                : '没有在用的实例，可以删。' })] })] })] })] })] })] })] }), _jsxs("div", { className: "grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_312px] overflow-hidden", children: [_jsxs("div", { className: "flex min-h-0 flex-col", children: [_jsxs("div", { className: "flex h-11 shrink-0 items-center gap-1 border-b border-line px-4", children: [_jsx("span", { className: "mr-1 shrink-0 text-1 font-semibold text-ink-3", children: "\u96F6\u4EF6" }), boot.palette.map((part) => (_jsxs("button", { type: "button", onPointerDown: (event) => startPaletteDrag(part, event), disabled: paletteDisabled, title: part.description + '。拖到线上', className: "flex h-7 shrink-0 cursor-grab items-center gap-1 rounded-btn px-1.5 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-35", children: [_jsx(FlowGlyph, { kind: part.nodeKind, size: 20 }), _jsx("span", { children: part.title })] }, part.kind))), _jsxs("div", { className: "ml-auto flex min-w-0 items-center gap-2 overflow-hidden pl-2 text-1 text-ink-3", children: [_jsx("span", { className: "shrink-0", children: "\u4EF7\u503C\u6D41" }), current.lines.map((line) => (_jsxs("span", { className: "flex shrink-0 items-center gap-1", children: [_jsx(LineChip, { flow: current, lineId: line.id }), _jsx("span", { className: "max-w-[72px] truncate text-ink-2", children: line.name })] }, line.id)))] })] }), _jsxs("div", { className: "relative shrink-0 px-4 pb-2 pt-1.5", children: [preview && proposal ? (_jsxs("div", { className: "flex min-h-[56px] flex-col justify-center gap-1", children: [_jsxs("div", { className: "flex min-w-0 items-center gap-2", children: [_jsx("span", { className: "grid size-6 shrink-0 place-items-center rounded-full bg-ink brand-type text-1 font-extrabold text-content", children: proposal.instanceId ? '实' : 'K' }), _jsx("b", { className: "shrink-0 text-2 font-semibold text-ink", children: proposal.instanceId ? '写回实例的改动' : 'Kind 的修改' }), _jsx("span", { className: "min-w-0 flex-1 truncate text-2 text-ink-2", title: proposal.said, children: proposal.said }), _jsxs("span", { className: "flex shrink-0 items-center gap-2 brand-type text-2 font-semibold", children: [_jsxs("span", { className: "text-done", children: ["+", countOps(proposal.ops).add, _jsx("span", { className: "ml-0.5 font-sans text-1 font-normal", children: "\u65B0\u52A0" })] }), _jsxs("span", { className: "text-fail", children: ["\u2212", countOps(proposal.ops).del, _jsx("span", { className: "ml-0.5 font-sans text-1 font-normal", children: "\u53BB\u6389" })] }), _jsxs("span", { className: "text-run", children: ["~", countOps(proposal.ops).chg, _jsx("span", { className: "ml-0.5 font-sans text-1 font-normal", children: "\u6539\u4E86" })] })] }), _jsx("button", { type: "button", onClick: rejectProposal, className: secondaryButton, children: "\u4E0D\u8981" }), _jsxs("button", { type: "button", onClick: acceptProposal, className: primaryButton, children: ["\u63A5\u53D7\uFF0C\u5B58\u4E3A v", current.version + 1] })] }), _jsxs("div", { className: "min-w-0 truncate pl-8 text-1 text-ink-3", children: [proposal.instanceId ? proposal.by + ' · ' : '', proposal.summary, " \u00B7 \u770B\u63D0\u8BAE\u65F6\u53EA\u8BFB"] })] })) : instanceMode ? (_jsxs("div", { className: "flex min-h-[56px] items-center gap-2 text-2 text-ink-2", children: [_jsx("span", { className: "grid size-6 shrink-0 place-items-center rounded-full bg-hover text-1 font-semibold text-ink-2", children: "\u5B9E" }), _jsxs("div", { className: "min-w-0", children: [_jsxs("div", { className: "truncate", children: [_jsx("b", { className: "font-semibold text-ink", children: "\u5B9E\u4F8B\u6001" }), activeInstance ? ' · 正在看 ' + activeInstance.name : ' · 这个流程还没有实例'] }), _jsx("div", { className: "truncate text-1 text-ink-3", children: "\u56FE\u4E0A\u7684\u70B9\u8868\u793A\u8FD9\u4E00\u8F6E\u8D70\u5230\u54EA\uFF1B\u8FD9\u91CC\u53EA\u8BFB\uFF0C\u56DE\u5230\u300C\u6D41\u7A0B\u300D\u518D\u6539\u3002" })] }), _jsx("button", { type: "button", onClick: () => setMode('flow'), className: cn(secondaryButton, 'ml-auto shrink-0'), children: "\u56DE\u5230\u6D41\u7A0B" })] })) : (_jsxs(_Fragment, { children: [_jsxs("form", { className: "flex items-center gap-2", onSubmit: (event) => {
                                                    event.preventDefault();
                                                    submitKind();
                                                }, children: [_jsx("span", { className: "grid size-6 shrink-0 place-items-center rounded-full bg-ink brand-type text-1 font-extrabold text-content", children: "K" }), selectedNode ? (_jsxs("span", { className: "shrink-0 rounded-badge bg-hover px-1.5 py-0.5 text-1 text-ink-2", children: ["\u6307\u7740\u300C", selectedNode.name, "\u300D"] })) : null, _jsx("input", { ref: kindInputRef, value: kindText, onChange: (event) => setKindText(event.target.value), disabled: kindDisabled, placeholder: selectedNode
                                                            ? '跟 Kind 说这一站怎么改…'
                                                            : '跟 Kind 说怎么改；先点一站，就是指着它说', "aria-label": "\u8DDF Kind \u8BF4", className: "h-8 min-w-0 flex-1 rounded-btn border border-line-2 bg-raised px-2.5 text-2 text-ink outline-none placeholder:text-ink-3 focus:border-run disabled:cursor-not-allowed disabled:opacity-55" }), _jsx("button", { type: "submit", disabled: kindDisabled || kindThinking || !kindText.trim(), className: primaryButton, children: kindThinking ? 'Kind 在看…' : '发给 Kind' })] }), _jsxs("div", { className: "mt-1 flex items-center gap-1.5 overflow-x-auto pl-8", children: [suggestions.map((suggestion) => (_jsx("button", { type: "button", onClick: () => submitKind(suggestion), disabled: kindDisabled || kindThinking, className: "h-6 shrink-0 rounded-full bg-hover px-2.5 text-1 text-ink-2 transition-colors hover:text-ink disabled:cursor-not-allowed disabled:opacity-45", children: suggestion }, suggestion))), _jsx("span", { className: "shrink-0 pl-1 text-1 text-ink-3", children: "\u6539\u52A8\u7528\u7EFF\u52A0 / \u7EA2\u53BB / \u84DD\u6539\u6807\u51FA\u6765" })] })] })), kindMsg ? (_jsxs("div", { className: "absolute left-4 right-4 top-full z-30 mt-1 flex items-start gap-2 rounded-card border border-line-2 bg-raised px-3 py-2 text-2 text-ink-2 shadow-[var(--shadow-pop)]", children: [_jsx("span", { className: "mt-0.5 grid size-5 shrink-0 place-items-center rounded-full bg-ink brand-type text-[9px] font-extrabold text-content", children: "K" }), _jsx("span", { className: "min-w-0 flex-1", children: kindMsg }), _jsx("button", { type: "button", onClick: () => setKindMsg(null), className: "grid size-5 shrink-0 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink", "aria-label": "\u5173\u4E0A Kind \u7684\u56DE\u590D", children: _jsx(X, { className: "size-3.5" }) })] })) : null] }), _jsx("div", { className: "min-h-0 flex-1", children: _jsx(FlowCanvas, { ref: canvasRef, flow: displayFlow, instance: instanceMode ? activeInstance : null, selectedIds: selectedIds, readOnly: preview || instanceMode || Boolean(current.archived), layoutAnimating: layoutAnimating, minimap: minimap, paletteDrag: paletteDrag, fitToken: fitToken, onMinimapChange: setMinimap, onSelectionChange: onCanvasSelection, onMoveNode: (id, position) => {
                                        if (preview || instanceMode || current.archived)
                                            return;
                                        setFlows((prev) => prev.map((flow) => flow.id === current.id
                                            ? {
                                                ...flow,
                                                nodes: flow.nodes.map((node) => (node.id === id ? { ...node, position } : node)),
                                            }
                                            : flow));
                                    }, onMoveStart: () => {
                                        if (preview || instanceMode || current.archived)
                                            return;
                                        pushUndo();
                                    }, onMoveEnd: persistDragEnd, onConnectNodes: onConnectNodes, onRejectConnection: showNotice, onDropPart: dropPart }) }), _jsx(IssuesPanel, { errors: validation.errors, infos: validation.infos, fixable: !preview && !instanceMode && !current.archived, preview: preview, onSelect: selectFromIssue, onFix: applyIssueFix, onFixAll: fixAll, onAck: acknowledge })] }), _jsx(FlowInspector, { flow: displayFlow, selectedIds: selectedIds, preview: preview, readOnly: instanceMode || Boolean(current.archived), archived: Boolean(current.archived), instance: instanceMode ? activeInstance : null, onPatchFlowName: patchFlowName, onPointToKind: () => kindInputRef.current?.focus(), onClose: () => setSelectedIds([]), onSelect: (id) => setSelectedIds(id ? [id] : []), onPatchNode: patchNode, onSetNodeKind: setNodeKind, onAddToArray: addToArray, onRemoveFromArray: removeFromArray, onAddDod: addDod, onPatchDod: patchDod, onRemoveDod: removeDod, onDeleteNode: (id) => deleteNodes([id]), onDeleteNodes: deleteNodes })] }), _jsx(InstancesPanel, { flow: current, disabled: preview || Boolean(current.archived), onUpgrade: upgradeInstance, onWriteBack: writeBackInstance }), _jsx(NewFlowDialog, { open: newOpen, onOpenChange: setNewOpen, onCreate: createFlow, templates: boot.templates }), paletteDrag
                ? createPortal(_jsxs("div", { className: "pointer-events-none fixed z-[120] flex items-center gap-1.5 rounded-card border border-line-2 bg-raised px-2 py-1 text-2 font-semibold text-ink shadow-[var(--shadow-pop)]", style: { left: paletteDrag.x + 14, top: paletteDrag.y + 12 }, children: [_jsx(FlowGlyph, { kind: partNodeKind(paletteDrag.part), size: 22 }), paletteDrag.title] }), document.body)
                : null, notice ? (_jsx("div", { role: "status", className: "pointer-events-none absolute bottom-3 left-1/2 z-40 max-w-[calc(100%-32px)] -translate-x-1/2 rounded-btn bg-ink px-3 py-1.5 text-2 text-content shadow-[var(--shadow-pop)]", children: notice })) : null] }));
}
