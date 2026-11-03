import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, } from 'react';
import { Background, BaseEdge, ConnectionLineType, ConnectionMode, Handle, MiniMap, Panel, Position, ReactFlow, ReactFlowProvider, SelectionMode, ViewportPortal, useReactFlow, useViewport, } from '@xyflow/react';
import { Clock, Map as MapIcon, Maximize2 } from 'lucide-react';
import { JOIN_LABEL } from '@/data/flows';
import { NODE_H, NODE_W, canConnect, canDropOnEdge, distanceToEdge, edgeLines, flowEdgePath, lineColor, primaryColor, projectOnEdge, snapToGrid } from './flowGraph';
import { FlowGlyph } from './flowGlyphs';
import { cn } from '@/lib/utils';
const EMPTY_GUIDES = { vertical: null, horizontal: null };
/* ───────────────────────── 节点 ───────────────────────── */
function initials(name) {
    if (name === 'Kind')
        return 'K';
    return [...name][0] ?? '?';
}
const STATUS_DOT = {
    run: 'bg-run',
    wait: 'bg-wait',
    done: 'bg-done',
    todo: 'bg-ink-3/50',
};
function nodeSubtitle(node) {
    if (node.kind === 'start')
        return node.rhythm || (node.start === 'cron' ? '到点开一轮' : '你说开始');
    if (node.kind === 'end')
        return '这一条线结束';
    if (node.kind === 'join')
        return node.join ? JOIN_LABEL[node.join] : '怎么汇合？';
    if (node.kind === 'pool')
        return '一进一出';
    if (node.kind === 'gate') {
        const count = (node.members ?? []).length;
        return count + ' 人 · ' + (node.rule || '怎么算过？');
    }
    if (node.kind === 'stack')
        return node.fan || '每条一遍';
    if (node.user)
        return '每次等你';
    if ((node.who ?? []).length)
        return node.who.join('、');
    return '还没定谁来做';
}
// 站名 15px、副标 12px；缩放到站名 ≥12px、副标 ≥11px 为止就不再缩小，
// 放不下的部分靠横向平移看（需求：不要整图缩到看不清）。
const MIN_ZOOM = 0.92;
function laneOffset(flow, lineId) {
    if (flow.lines.length <= 1)
        return 0;
    const index = Math.max(0, flow.lines.findIndex((line) => line.id === lineId));
    return (index - (flow.lines.length - 1) / 2) * 6;
}
function FlowNodeView({ data }) {
    const { node, lines, status, connectable, selected, dimmed } = data;
    const diff = node.diff;
    const diffLabel = diff === 'add' ? '新加' : diff === 'del' ? '去掉' : diff === 'chg' ? '改了' : null;
    const sub = nodeSubtitle(node);
    const members = node.kind === 'gate' ? (node.members ?? []).slice(0, 3) : [];
    const hasLoop = Boolean(node.loop);
    const hasRhythm = Boolean(node.rhythm) && node.kind !== 'start';
    const showAvatars = members.length > 0;
    const showLineChips = lines.length > 1 || node.kind === 'start';
    // 地铁线规则（brief §四.2）：站是中性。跨线站按 6px 线距撑开成胶囊 / 跨线菱形，
    // 高度最多 28px，不压到站名（站名从 top-[58px] 开始）。
    const transfer = lines.length > 1;
    const spanH = Math.min(28, 18 + 6 * (lines.length - 1));
    const diamondW = Math.round(spanH * 0.7);
    const capsuleGlyph = transfer && node.kind === 'station';
    const diamondGlyph = transfer && node.kind === 'gate';
    const glyphW = capsuleGlyph ? 18 : diamondGlyph ? diamondW : 26;
    const glyphH = capsuleGlyph || diamondGlyph ? spanH : 26;
    // 状态点贴在站形右上沿：跨线胶囊/菱形按边角，普通零件按各自形状的外沿。
    const dotX = capsuleGlyph
        ? glyphW / 2
        : diamondGlyph
            ? glyphW / 4
            : node.kind === 'station'
                ? 5.6
                : node.kind === 'gate'
                    ? 5.1
                    : node.kind === 'pool'
                        ? 7.3
                        : node.kind === 'stack'
                            ? 9.7
                            : node.kind === 'join'
                                ? 4.9
                                : node.start === 'cron'
                                    ? 6.9
                                    : 2;
    const dotY = capsuleGlyph
        ? -glyphH / 2
        : diamondGlyph
            ? -glyphH / 4
            : node.kind === 'station'
                ? -5.6
                : node.kind === 'gate'
                    ? -5.1
                    : node.kind === 'pool'
                        ? -4.9
                        : node.kind === 'stack'
                            ? -9.7
                            : node.kind === 'join'
                                ? -4.9
                                : -8.9;
    const textShadow = '0 0 3px var(--content), 0 0 4px var(--content), 0 0 6px var(--content)';
    const chips = showLineChips
        ? lines.map((line) => (_jsx("span", { className: "inline-grid size-[11px] place-items-center rounded-[3px] brand-type text-[8px] font-bold leading-none text-white", style: { background: line.color }, children: line.index }, line.index)))
        : null;
    const avatars = showAvatars
        ? members.map((member) => (_jsx("span", { className: "grid size-[11px] place-items-center rounded-full bg-raised text-[6px] font-semibold text-ink-2 ring-1 ring-line-2", children: initials(member) }, member)))
        : null;
    const extras = hasLoop ? (_jsxs("span", { className: "rounded-[3px] border border-line-2 bg-content px-0.5 brand-type text-[8px] leading-[10px] text-ink-3", children: ["\u2264", node.loop?.max ?? '?'] })) : hasRhythm ? (_jsx(Clock, { className: "size-2.5 text-ink-3", "aria-hidden": "true" })) : null;
    return (_jsxs("div", { className: cn('relative h-[88px] w-20 select-none text-center transition-opacity', diff === 'del' && 'opacity-55', dimmed && 'opacity-25'), title: node.name + ' · ' + (node.kind === 'station' ? (transfer ? '换乘站' : '站') : node.kind === 'stack' ? '逐条展开' : node.kind === 'gate' ? '门' : node.kind === 'pool' ? '池' : node.kind === 'join' ? '汇合' : node.kind === 'start' ? '起点' : '终点'), children: [selected ? (_jsx("span", { className: "pointer-events-none absolute -inset-[5px] rounded-xl border border-dashed border-ink/55" })) : null, diff ? (_jsx("span", { className: cn('pointer-events-none absolute -inset-[5px] rounded-xl border', diff === 'add' && 'border-done', diff === 'del' && 'border-fail border-dashed', diff === 'chg' && 'border-run') })) : null, diffLabel ? (_jsx("span", { className: cn('pointer-events-none absolute -top-[15px] left-1/2 -translate-x-1/2 whitespace-nowrap text-[9px] font-semibold', diff === 'add' && 'text-done', diff === 'del' && 'text-fail', diff === 'chg' && 'text-run'), children: diffLabel })) : null, _jsx("div", { className: "absolute inset-x-0 top-0.5 z-[2] flex h-3 items-center gap-0.5 px-0.5", children: showAvatars ? (_jsxs(_Fragment, { children: [_jsx("span", { className: "flex items-center gap-[2px]", children: chips }), _jsxs("span", { className: "ml-auto flex items-center gap-[2px]", children: [avatars, extras] })] })) : (_jsxs(_Fragment, { children: [_jsx("span", { className: "flex flex-1 items-center justify-center gap-[2px]", children: chips }), extras ? _jsx("span", { className: "absolute right-0.5 flex items-center", children: extras }) : null] })) }), _jsxs("div", { className: "absolute left-1/2 top-1/2 z-[1] -translate-x-1/2 -translate-y-1/2", children: [transfer && node.kind === 'station' ? (_jsxs("svg", { width: 18, height: spanH, viewBox: '0 0 18 ' + spanH, className: "overflow-visible", "aria-hidden": "true", children: [_jsx("title", { children: "\u6362\u4E58\u7AD9\uFF1A\u51E0\u6761\u4EF7\u503C\u6D41\u5171\u7528\u8FD9\u4E00\u7AD9" }), _jsx("rect", { x: "1", y: "1", width: 16, height: spanH - 2, rx: 6, fill: "var(--stn-fill)", stroke: "var(--stn-line)", strokeWidth: "2" })] })) : transfer && node.kind === 'gate' ? (_jsxs("svg", { width: diamondW, height: spanH, viewBox: '0 0 ' + diamondW + ' ' + spanH, className: "overflow-visible", "aria-hidden": "true", children: [_jsx("title", { children: "\u8DE8\u7EBF\u95E8\uFF1A\u51E0\u6761\u4EF7\u503C\u6D41\u5171\u7528\u8FD9\u4E00\u9053\u95E8" }), _jsx("path", { d: 'M ' + diamondW / 2 + ' 1 L ' + (diamondW - 1) + ' ' + spanH / 2 + ' L ' + diamondW / 2 + ' ' + (spanH - 1) + ' L 1 ' + spanH / 2 + ' Z', fill: "var(--stn-fill)", stroke: "var(--stn-line)", strokeWidth: "2", strokeLinejoin: "round" })] })) : (_jsx(FlowGlyph, { kind: node.kind, start: node.start, size: 26, className: "overflow-visible" })), status ? (_jsx("span", { className: cn('absolute z-10 size-[7px] rounded-full ring-2 ring-content', STATUS_DOT[status]), style: { left: glyphW / 2 + dotX - 3.5, top: glyphH / 2 + dotY - 3.5 }, title: status === 'run' ? '进行中' : status === 'wait' ? '等你' : status === 'done' ? '完成' : '还没开始' })) : null] }), _jsx("div", { className: "absolute left-1/2 top-[58px] w-[104px] -translate-x-1/2 truncate text-[15px] font-semibold leading-[17px] text-ink", style: { textShadow }, children: node.name }), _jsx("div", { className: cn('absolute left-1/2 top-[75px] w-[104px] -translate-x-1/2 truncate text-[12px] leading-[14px]', node.user && node.kind !== 'start' && node.kind !== 'end' ? 'text-wait' : 'text-ink-3'), style: { textShadow }, title: sub, children: sub }), node.kind !== 'start' ? (_jsx(Handle, { type: "target", position: Position.Left, id: "in", isConnectable: connectable, title: connectable ? '从这里接进来' : undefined, className: cn('!size-[7px] !border-2 !border-content !bg-ink-3 hover:!bg-run', !connectable && '!pointer-events-none !opacity-0') })) : null, node.kind !== 'end' ? (_jsx(Handle, { type: "source", position: Position.Right, id: "out", isConnectable: connectable, title: connectable ? '从这里连出去' : undefined, className: cn('!size-[7px] !border-2 !border-content !bg-ink-3 hover:!bg-run', !connectable && '!pointer-events-none !opacity-0') })) : null] }));
}
/* ───────────────────────── 边 ───────────────────────── */
function FlowEdgeView({ id, data }) {
    if (!data)
        return null;
    const path = flowEdgePath(data.sx, data.sy, data.tx, data.ty);
    const diff = data.diff;
    const width = data.width ?? 3.5;
    const color = diff === 'add' ? 'var(--done)' : diff === 'del' ? 'var(--fail)' : data.color;
    return (_jsxs("g", { children: [diff === 'add' ? (_jsx(BaseEdge, { id: id + '-halo', path: path, style: { stroke: color, strokeWidth: width * 3, opacity: 0.16 } })) : null, _jsx(BaseEdge, { id: id + '-casing', path: path, style: { stroke: 'var(--content)', strokeWidth: width + 1, opacity: data.dimmed ? 0.15 : 1 } }), _jsx(BaseEdge, { id: id, path: path, style: {
                    stroke: color,
                    strokeWidth: width,
                    strokeLinecap: 'round',
                    strokeLinejoin: 'round',
                    strokeDasharray: diff === 'del' ? '6 5' : undefined,
                    opacity: data.dimmed ? 0.25 : diff === 'del' ? 0.8 : 1,
                } })] }));
}
const nodeTypes = { flow: FlowNodeView };
const edgeTypes = { flow: FlowEdgeView };
/* ───────────────────────── 参考线 ───────────────────────── */
const ALIGN_THRESHOLD = 6;
function findVertical(x, nodeY, draggedId, nodes) {
    let best = null;
    const dragged = [x, x + NODE_W / 2, x + NODE_W];
    for (const other of nodes) {
        if (other.id === draggedId)
            continue;
        const oy = other.position.y;
        const others = [other.position.x, other.position.x + NODE_W / 2, other.position.x + NODE_W];
        for (let i = 0; i < dragged.length; i += 1) {
            for (let j = 0; j < others.length; j += 1) {
                const diff = others[j] - dragged[i];
                const distance = Math.abs(diff);
                if (distance > ALIGN_THRESHOLD)
                    continue;
                if (best && distance >= best.distance)
                    continue;
                best = {
                    guide: others[j],
                    x: x + diff,
                    distance,
                    from: Math.min(nodeY, oy) - 18,
                    to: Math.max(nodeY + NODE_H, oy + NODE_H) + 18,
                };
            }
        }
    }
    if (!best)
        return null;
    let from = Number.POSITIVE_INFINITY;
    let to = Number.NEGATIVE_INFINITY;
    for (const other of nodes) {
        from = Math.min(from, other.position.y - 18);
        to = Math.max(to, other.position.y + NODE_H + 18);
    }
    return {
        guide: best.guide,
        x: best.x,
        from: Number.isFinite(from) ? from : best.from,
        to: Number.isFinite(to) ? to : best.to,
    };
}
function findHorizontal(y, nodeX, draggedId, nodes) {
    let best = null;
    const dragged = [y, y + NODE_H / 2, y + NODE_H];
    for (const other of nodes) {
        if (other.id === draggedId)
            continue;
        const ox = other.position.x;
        const others = [other.position.y, other.position.y + NODE_H / 2, other.position.y + NODE_H];
        for (let i = 0; i < dragged.length; i += 1) {
            for (let j = 0; j < others.length; j += 1) {
                const diff = others[j] - dragged[i];
                const distance = Math.abs(diff);
                if (distance > ALIGN_THRESHOLD)
                    continue;
                if (best && distance >= best.distance)
                    continue;
                best = {
                    guide: others[j],
                    y: y + diff,
                    distance,
                    from: Math.min(nodeX, ox) - 18,
                    to: Math.max(nodeX + NODE_W, ox + NODE_W) + 18,
                };
            }
        }
    }
    if (!best)
        return null;
    let from = Number.POSITIVE_INFINITY;
    let to = Number.NEGATIVE_INFINITY;
    for (const other of nodes) {
        from = Math.min(from, other.position.x - 18);
        to = Math.max(to, other.position.x + NODE_W + 18);
    }
    return {
        guide: best.guide,
        y: best.y,
        from: Number.isFinite(from) ? from : best.from,
        to: Number.isFinite(to) ? to : best.to,
    };
}
function CanvasInner(props) {
    const { flow, instance, selectedIds, readOnly, layoutAnimating, minimap, paletteDrag, fitToken, onMinimapChange, onSelectionChange, onMoveNode, onMoveStart, onMoveEnd, onConnectNodes, onRejectConnection, onDropPart, handleRef, } = props;
    const wrapperRef = useRef(null);
    const { screenToFlowPosition, fitView, getNodes, getViewport, setViewport } = useReactFlow();
    /**
     * 适配视图。缩到 MIN_ZOOM 还放不下时，fitView 会把内容居中、两头都切掉；
     * 这里改成贴左显示（起点可见），剩下的靠横向平移看。
     */
    const fitGraph = useCallback(async (duration = 180) => {
        await fitView({ padding: 0.08, duration: reduceMotion() ? 0 : duration, maxZoom: 1.2, minZoom: MIN_ZOOM });
        const viewport = getViewport();
        if (viewport.zoom > MIN_ZOOM + 0.005)
            return;
        const nodes = getNodes();
        if (!nodes.length)
            return;
        let minX = Number.POSITIVE_INFINITY;
        for (const node of nodes)
            minX = Math.min(minX, node.position.x);
        if (!Number.isFinite(minX))
            return;
        setViewport({ x: 24 - minX * viewport.zoom, y: viewport.y, zoom: viewport.zoom }, { duration: reduceMotion() ? 0 : duration });
    }, [fitView, getNodes, getViewport, setViewport]);
    const { zoom } = useViewport();
    const [guides, setGuides] = useState(EMPTY_GUIDES);
    // 图例只看这一条（brief §四.2：≥4 条线时点图例里的一条，其余淡到 25%）
    const [activeLine, setActiveLine] = useState(null);
    const activeLineId = flow.lines.length >= 4 ? activeLine : null;
    useEffect(() => {
        setActiveLine(null);
    }, [flow.id]);
    const moveStartedRef = useRef(false);
    const dragOriginRef = useRef(null);
    const selectionActiveRef = useRef(false);
    const selectionDraftRef = useRef(null);
    const selectedIdsRef = useRef(selectedIds);
    selectedIdsRef.current = selectedIds;
    const reduceMotion = () => document.documentElement.dataset.reducedMotion === 'true';
    const lineBadges = useMemo(() => {
        const map = new Map();
        for (const node of flow.nodes) {
            map.set(node.id, node.lines
                .map((id) => flow.lines.find((line) => line.id === id))
                .filter((line) => Boolean(line))
                .map((line) => ({ index: line.index, color: line.color })));
        }
        return map;
    }, [flow.lines, flow.nodes]);
    const rfNodes = useMemo(() => {
        return flow.nodes.map((node) => {
            return {
                id: node.id,
                type: 'flow',
                position: node.position,
                data: {
                    node,
                    lineColor: primaryColor(flow, node),
                    lines: lineBadges.get(node.id) ?? [],
                    status: instance?.nodeStates?.[node.id] ?? null,
                    connectable: !readOnly,
                    selected: selectedIds.includes(node.id),
                    dimmed: activeLineId ? !node.lines.includes(activeLineId) : false,
                },
                draggable: !readOnly,
                connectable: !readOnly,
                deletable: false,
                width: NODE_W,
                height: NODE_H,
                className: layoutAnimating
                    ? 'transition-transform duration-[180ms] ease-[cubic-bezier(0.2,0.8,0.2,1)]'
                    : undefined,
                style: { width: NODE_W, height: NODE_H },
            };
        });
    }, [flow, instance, layoutAnimating, lineBadges, readOnly, selectedIds, activeLineId]);
    const rfEdges = useMemo(() => {
        const map = new Map(flow.nodes.map((node) => [node.id, node]));
        const out = [];
        const strokeWidth = 4;
        for (const edge of flow.edges) {
            const source = map.get(edge.source);
            const target = map.get(edge.target);
            // 共用路段：一条逻辑连线，按 lineIds 画成几条并排的线（错开 laneOffset）
            const lineIds = edge.lineIds && edge.lineIds.length > 0 ? edge.lineIds : [edge.lineId];
            for (const lineId of lineIds) {
                const offset = laneOffset(flow, lineId);
                out.push({
                    id: lineIds.length > 1 ? edge.id + '#' + lineId : edge.id,
                    source: edge.source,
                    target: edge.target,
                    type: 'flow',
                    data: {
                        color: lineColor(flow, lineId),
                        diff: edge.diff,
                        dimmed: activeLineId ? lineId !== activeLineId : false,
                        width: strokeWidth,
                        sx: source ? source.position.x + NODE_W / 2 : 0,
                        sy: source ? source.position.y + NODE_H / 2 + offset : 0,
                        tx: target ? target.position.x + NODE_W / 2 : 0,
                        ty: target ? target.position.y + NODE_H / 2 + offset : 0,
                    },
                    selectable: false,
                    focusable: false,
                    deletable: false,
                });
            }
        }
        return out;
    }, [flow, activeLineId]);
    const lines = useMemo(() => edgeLines(flow), [flow]);
    const hover = useMemo(() => {
        if (!paletteDrag)
            return null;
        const point = screenToFlowPosition({ x: paletteDrag.x, y: paletteDrag.y });
        const threshold = 24 / Math.max(0.3, zoom);
        let nearest = null;
        let nearestDistance = Number.POSITIVE_INFINITY;
        for (const line of lines) {
            const distance = distanceToEdge(point, line);
            if (distance < nearestDistance) {
                nearestDistance = distance;
                nearest = line;
            }
        }
        const inRadius = Boolean(nearest && nearestDistance <= threshold);
        return {
            point,
            line: inRadius ? nearest : null,
            check: nearest && inRadius ? canDropOnEdge(flow, paletteDrag.part, nearest.edge.id) : null,
            validEdges: lines.filter((line) => canDropOnEdge(flow, paletteDrag.part, line.edge.id).ok),
        };
    }, [flow, lines, paletteDrag, screenToFlowPosition, zoom]);
    const tip = useMemo(() => {
        if (!paletteDrag)
            return '';
        if (paletteDrag.part === 'join')
            return '汇合拖到空白处，再把几支连进来';
        if (hover?.line && hover.check && !hover.check.ok)
            return hover.check.reason ?? '这里放不了';
        if (hover?.line)
            return '放这里，线会一分为二';
        if (paletteDrag.part === 'pool')
            return '池只能放在主线上、两站之间';
        return '放到线上会插在中间；放到空白处会成为没连上的站';
    }, [hover, paletteDrag]);
    useEffect(() => {
        const timer = window.setTimeout(() => {
            void fitGraph();
        }, 30);
        return () => window.clearTimeout(timer);
    }, [fitView, fitToken, flow.id]);
    const onNodeDragStart = useCallback((_event, node) => {
        moveStartedRef.current = false;
        dragOriginRef.current = { x: node.position.x, y: node.position.y };
        setGuides(EMPTY_GUIDES);
    }, []);
    const onNodeDrag = useCallback((_event, node) => {
        if (readOnly)
            return;
        if (!moveStartedRef.current) {
            const origin = dragOriginRef.current;
            const moved = !origin || Math.abs(node.position.x - origin.x) > 1 || Math.abs(node.position.y - origin.y) > 1;
            if (!moved)
                return;
            moveStartedRef.current = true;
            onMoveStart();
        }
        let x = snapToGrid(node.position.x);
        let y = snapToGrid(node.position.y);
        const vertical = findVertical(x, y, node.id, flow.nodes);
        const horizontal = findHorizontal(y, x, node.id, flow.nodes);
        if (vertical)
            x = vertical.x;
        if (horizontal)
            y = horizontal.y;
        setGuides({
            vertical: vertical ? { x: vertical.guide, from: vertical.from, to: vertical.to } : null,
            horizontal: horizontal ? { y: horizontal.guide, from: horizontal.from, to: horizontal.to } : null,
        });
        onMoveNode(node.id, { x: Math.round(x), y: Math.round(y) });
    }, [flow.nodes, onMoveNode, onMoveStart, readOnly]);
    const onNodeDragStop = useCallback(() => {
        setGuides(EMPTY_GUIDES);
        if (moveStartedRef.current)
            onMoveEnd();
        moveStartedRef.current = false;
        dragOriginRef.current = null;
    }, [onMoveEnd]);
    const onNodesChange = useCallback((changes) => {
        const selects = changes.filter((change) => change.type === 'select');
        if (!selects.length)
            return;
        if (selectionActiveRef.current) {
            const draft = new Set(selectionDraftRef.current ?? selectedIdsRef.current);
            for (const change of selects) {
                if (change.selected)
                    draft.add(change.id);
                else
                    draft.delete(change.id);
            }
            selectionDraftRef.current = [...draft];
            onSelectionChange([...draft]);
        }
        else {
            const selected = selects.filter((change) => change.selected).map((change) => change.id);
            onSelectionChange(selected.length ? [selected[selected.length - 1]] : []);
        }
    }, [onSelectionChange]);
    const isValidConnection = useCallback((connection) => {
        if (readOnly)
            return false;
        if (!connection.source || !connection.target)
            return false;
        return canConnect(flow, connection.source, connection.target).ok;
    }, [flow, readOnly]);
    const onConnect = useCallback((connection) => {
        if (!connection.source || !connection.target)
            return;
        onConnectNodes(connection.source, connection.target);
    }, [onConnectNodes]);
    const onConnectEnd = useCallback((_event, state) => {
        if (!state.fromNode || !state.toNode || state.isValid)
            return;
        const check = canConnect(flow, state.fromNode.id, state.toNode.id);
        if (!check.ok && check.reason)
            onRejectConnection(check.reason);
    }, [flow, onRejectConnection]);
    const dropPart = useCallback((part, clientX, clientY) => {
        const wrapper = wrapperRef.current;
        if (!wrapper)
            return { ok: false };
        const rect = wrapper.getBoundingClientRect();
        if (clientX < rect.left || clientX > rect.right || clientY < rect.top || clientY > rect.bottom) {
            return { ok: false };
        }
        const point = screenToFlowPosition({ x: clientX, y: clientY });
        const threshold = 24 / Math.max(0.3, zoom);
        let nearest = null;
        let nearestDistance = Number.POSITIVE_INFINITY;
        for (const line of lines) {
            const distance = distanceToEdge(point, line);
            if (distance < nearestDistance) {
                nearestDistance = distance;
                nearest = line;
            }
        }
        if (nearest && nearestDistance <= threshold && part !== 'join') {
            const check = canDropOnEdge(flow, part, nearest.edge.id);
            if (!check.ok)
                return { ok: false, reason: check.reason };
            const projected = projectOnEdge(point, nearest);
            const position = {
                x: snapToGrid(projected.x - NODE_W / 2),
                y: snapToGrid(projected.y - NODE_H / 2),
            };
            return onDropPart(part, position, nearest.edge.id);
        }
        const position = {
            x: snapToGrid(point.x - NODE_W / 2),
            y: snapToGrid(point.y - NODE_H / 2),
        };
        return onDropPart(part, position, null);
    }, [flow, lines, onDropPart, screenToFlowPosition, zoom]);
    useImperativeHandle(handleRef, () => ({
        dropPart,
        fit() {
            void fitGraph();
        },
    }), [dropPart, fitView]);
    return (_jsx("div", { ref: wrapperRef, className: "relative h-full min-h-0 w-full overflow-hidden bg-content", children: _jsxs(ReactFlow, { nodes: rfNodes, edges: rfEdges, nodeTypes: nodeTypes, edgeTypes: edgeTypes, onNodeDragStart: onNodeDragStart, onNodeDrag: onNodeDrag, onNodeDragStop: onNodeDragStop, onNodesChange: onNodesChange, onSelectionStart: () => {
                selectionActiveRef.current = true;
                selectionDraftRef.current = [];
                onSelectionChange([]);
            }, onSelectionEnd: () => {
                selectionActiveRef.current = false;
                selectionDraftRef.current = null;
            }, isValidConnection: isValidConnection, onConnect: onConnect, onConnectEnd: onConnectEnd, nodesDraggable: !readOnly, nodesConnectable: !readOnly, nodesFocusable: !readOnly, edgesFocusable: false, elementsSelectable: true, panOnDrag: true, selectionKeyCode: "Shift", multiSelectionKeyCode: ['Meta', 'Control'], selectionMode: SelectionMode.Partial, deleteKeyCode: null, connectionMode: ConnectionMode.Strict, connectionRadius: 26, connectionLineType: ConnectionLineType.SmoothStep, connectionLineStyle: { stroke: 'var(--ink-3)', strokeWidth: 1.6, strokeDasharray: '4 4' }, zoomOnDoubleClick: false, minZoom: MIN_ZOOM, maxZoom: 1.8, fitView: true, fitViewOptions: { padding: 0.08, maxZoom: 1.2, minZoom: MIN_ZOOM }, proOptions: { hideAttribution: true }, className: "flow-canvas", children: [_jsx(Background, { color: "var(--line-2)", gap: 16, size: 1 }), minimap ? (_jsx(MiniMap, { pannable: true, zoomable: true, position: "bottom-right", maskColor: "var(--content)", nodeColor: (node) => node.data.lineColor, nodeStrokeColor: "var(--content)", style: { width: 150, height: 94, background: 'var(--pane)', border: '1px solid var(--line)' } })) : null, _jsxs(Panel, { position: "top-right", className: "!m-2 flex gap-1", children: [_jsxs("button", { type: "button", onClick: () => onMinimapChange(!minimap), "aria-pressed": minimap, className: cn('inline-flex h-7 items-center gap-1 rounded-btn border border-line-2 bg-content/90 px-2 text-2 backdrop-blur transition-colors', minimap ? 'text-ink' : 'text-ink-3 hover:text-ink'), title: "\u5C0F\u5730\u56FE\u5F00\u5173", children: [_jsx(MapIcon, { className: "size-3.5" }), "\u5C0F\u5730\u56FE"] }), _jsx("button", { type: "button", onClick: () => void fitGraph(), className: "grid size-7 place-items-center rounded-btn border border-line-2 bg-content/90 text-ink-2 backdrop-blur transition-colors hover:text-ink", title: "\u9002\u5E94\u89C6\u56FE", "aria-label": "\u9002\u5E94\u89C6\u56FE", children: _jsx(Maximize2, { className: "size-3.5" }) })] }), flow.lines.length >= 4 ? (_jsx(Panel, { position: "top-left", className: "!m-2", children: _jsxs("div", { className: "flex flex-col gap-0.5 rounded-btn border border-line-2 bg-content/85 p-1.5 text-1 backdrop-blur", children: [_jsx("span", { className: "px-1 pb-0.5 text-ink-3", children: "\u4EF7\u503C\u6D41 \u00B7 \u70B9\u4E00\u6761\u53EA\u770B\u5B83" }), flow.lines.map((line) => (_jsxs("button", { type: "button", onClick: () => setActiveLine((previous) => (previous === line.id ? null : line.id)), "aria-pressed": activeLine === line.id, className: cn('flex items-center gap-1.5 rounded-badge px-1 py-1 text-left transition-colors', activeLine === line.id ? 'bg-hover text-ink' : 'text-ink-2 hover:bg-hover', activeLine && activeLine !== line.id && 'opacity-40'), children: [_jsx("span", { className: "h-[3px] w-3 shrink-0 rounded-full", style: { background: line.color } }), _jsx("span", { className: "truncate", children: line.name })] }, line.id))), activeLine ? (_jsx("button", { type: "button", onClick: () => setActiveLine(null), className: "mt-0.5 px-1 text-left text-ink-3 underline decoration-dotted", children: "\u663E\u793A\u5168\u90E8" })) : null] }) })) : null, _jsx(Panel, { position: "bottom-left", className: "!m-2", children: _jsx("span", { className: "pointer-events-none rounded-btn border border-line bg-content/80 px-2 py-1 text-1 text-ink-3 backdrop-blur", children: "\u62D6\u52A8\u7A7A\u767D\u5E73\u79FB \u00B7 Shift \u62D6\u52A8\u6846\u9009 \u00B7 \u6EDA\u8F6E\u7F29\u653E \u00B7 \u5438\u9644 16" }) }), paletteDrag && tip ? (_jsx(Panel, { position: "bottom-center", className: "!m-2", children: _jsx("span", { className: cn('pointer-events-none rounded-btn border px-2.5 py-1 text-2 shadow-[var(--shadow-pop)] backdrop-blur', hover?.line && hover.check && !hover.check.ok
                            ? 'border-fail/50 bg-fail-soft text-fail'
                            : 'border-line-2 bg-raised text-ink-2'), children: tip }) })) : null, _jsx(ViewportPortal, { children: _jsxs("svg", { className: "pointer-events-none absolute left-0 top-0 overflow-visible", width: "1", height: "1", "aria-hidden": "true", children: [guides.vertical ? (_jsx("line", { x1: guides.vertical.x, x2: guides.vertical.x, y1: guides.vertical.from, y2: guides.vertical.to, stroke: "var(--ink-3)", strokeWidth: "1", strokeDasharray: "4 4", vectorEffect: "non-scaling-stroke" })) : null, guides.horizontal ? (_jsx("line", { x1: guides.horizontal.from, x2: guides.horizontal.to, y1: guides.horizontal.y, y2: guides.horizontal.y, stroke: "var(--ink-3)", strokeWidth: "1", strokeDasharray: "4 4", vectorEffect: "non-scaling-stroke" })) : null, paletteDrag
                                ? hover?.validEdges.map((line) => {
                                    const source = flow.nodes.find((node) => node.id === line.edge.source);
                                    const target = flow.nodes.find((node) => node.id === line.edge.target);
                                    if (!source || !target)
                                        return null;
                                    const offset = laneOffset(flow, line.edge.lineId);
                                    const path = flowEdgePath(source.position.x + NODE_W / 2, source.position.y + NODE_H / 2 + offset, target.position.x + NODE_W / 2, target.position.y + NODE_H / 2 + offset);
                                    const isHot = hover.line?.edge.id === line.edge.id;
                                    return (_jsx("path", { d: path, fill: "none", stroke: lineColor(flow, line.edge.lineId), strokeWidth: isHot ? 13 : 10, strokeLinecap: "round", opacity: isHot ? 0.5 : 0.26 }, line.edge.id));
                                })
                                : null, paletteDrag && hover?.line && hover.check?.ok ? (_jsx("g", { transform: 'translate(' + projectOnEdge(hover.point, hover.line).x + ' ' + projectOnEdge(hover.point, hover.line).y + ')', children: _jsx("circle", { r: "6", fill: "var(--run)", stroke: "var(--content)", strokeWidth: "2" }) })) : null] }) })] }) }));
}
export const FlowCanvas = forwardRef(function FlowCanvas(props, ref) {
    return (_jsx(ReactFlowProvider, { children: _jsx(CanvasInner, { ...props, handleRef: ref }) }));
});
