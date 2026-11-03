/**
 * 流程图业务逻辑：检查、修复、连线、Kind 提议、elk 排版、位置存取。
 * 只有这里改图；组件只负责显示和把动作转成调用。
 */
import { JOIN_LABEL } from '@/data/flows';
export const NODE_W = 80;
export const NODE_H = 88;
export const GRID = 16;
const STATION_LIKE = new Set(['station', 'stack']);
const TERMINAL_LIKE = new Set(['start', 'end', 'join']);
/* ───────────────────────── 基础 ───────────────────────── */
let seq = 0;
export function nextId(prefix) {
    seq += 1;
    return prefix + '-' + Date.now().toString(36) + '-' + seq.toString(36);
}
export function clone(value) {
    return JSON.parse(JSON.stringify(value));
}
export function cloneFlow(flow) {
    return clone(flow);
}
export function sameGraph(a, b) {
    return JSON.stringify(a) === JSON.stringify(b);
}
export function lineById(flow, lineId) {
    return flow.lines.find((line) => line.id === lineId);
}
export function lineColor(flow, lineId) {
    return lineById(flow, lineId)?.color ?? 'var(--ink-2)';
}
export function primaryColor(flow, node) {
    return lineColor(flow, node.lines[0] ?? flow.lines[0]?.id ?? '');
}
export function isStationLike(kind) {
    return STATION_LIKE.has(kind);
}
export function isShared(flow, node) {
    return node.lines.filter((id) => flow.lines.some((line) => line.id === id)).length > 1;
}
export function nodeCenter(node) {
    return { x: node.position.x + NODE_W / 2, y: node.position.y + NODE_H / 2 };
}
export function snapToGrid(value) {
    return Math.round(value / GRID) * GRID;
}
export function nodeSnapshot(flow) {
    return { nodes: clone(flow.nodes), edges: clone(flow.edges) };
}
function outgoing(flow, nodeId) {
    return flow.edges.filter((edge) => edge.source === nodeId);
}
function incoming(flow, nodeId) {
    return flow.edges.filter((edge) => edge.target === nodeId);
}
function nodeMap(flow) {
    const map = new Map();
    for (const node of flow.nodes)
        map.set(node.id, node);
    return map;
}
/* ───────────────────────── 建零件 ───────────────────────── */
export function createNode(part, position, lineId) {
    const base = { id: nextId('n'), lines: [lineId], position: { x: snapToGrid(position.x), y: snapToGrid(position.y) } };
    switch (part) {
        case 'user':
            return {
                ...base,
                kind: 'station',
                name: '等你确认',
                who: ['你'],
                user: true,
                skills: [],
                recv: '',
                give: '你的确认',
                dod: [{ text: '你点了头', auto: false }],
            };
        case 'stack':
            return {
                ...base,
                kind: 'stack',
                name: '逐条做',
                fan: '每条一遍',
                who: [],
                skills: [],
                recv: '',
                give: '',
                dod: [],
            };
        case 'gate':
            return {
                ...base,
                kind: 'gate',
                name: '审读门',
                members: ['审读'],
                rule: '全部通过',
                fail: '退回上一站改。',
                recv: '',
                give: '过审的结果',
                dod: [],
            };
        case 'pool':
            return {
                ...base,
                kind: 'pool',
                name: '池',
                note: '上游攒着，下游按自己的节奏来取。',
                recv: '',
                give: '',
            };
        case 'join':
            return { ...base, kind: 'join', name: '汇合', join: 'all', recv: '', give: '' };
        case 'station':
        default:
            return {
                ...base,
                kind: 'station',
                name: '新的一站',
                who: [],
                skills: [],
                recv: '',
                give: '',
                dod: [],
            };
    }
}
/* ───────────────────────── 图上动作 ───────────────────────── */
export function insertNodeOnEdge(flow, edgeId, node) {
    const next = cloneFlow(flow);
    const index = next.edges.findIndex((edge) => edge.id === edgeId);
    if (index < 0) {
        next.nodes.push(node);
        return next;
    }
    const edge = next.edges[index];
    const source = next.nodes.find((item) => item.id === edge.source);
    const target = next.nodes.find((item) => item.id === edge.target);
    if (source && target && Math.abs(source.position.y - target.position.y) < 1) {
        // 插在横向的线上：把目标及其下游往右挪一格，新站正好占原来的位置。
        const oldPosition = { ...target.position };
        const downstream = forwardSet(next, target.id);
        for (const item of next.nodes) {
            if (downstream.has(item.id)) {
                item.position = { ...item.position, x: item.position.x + NODE_W + 28 };
            }
        }
        node.position = oldPosition;
    }
    next.edges.splice(index, 1);
    next.nodes.push(node);
    next.edges.push({ id: nextId('e'), source: edge.source, target: node.id, lineId: edge.lineId });
    next.edges.push({ id: nextId('e'), source: node.id, target: edge.target, lineId: edge.lineId });
    ensureLine(next, edge.source, edge.lineId);
    ensureLine(next, edge.target, edge.lineId);
    return next;
}
export function removeNode(flow, nodeId) {
    const next = cloneFlow(flow);
    next.nodes = next.nodes.filter((node) => node.id !== nodeId);
    next.edges = next.edges.filter((edge) => edge.source !== nodeId && edge.target !== nodeId);
    return next;
}
export function removeEdge(flow, edgeId) {
    const next = cloneFlow(flow);
    next.edges = next.edges.filter((edge) => edge.id !== edgeId);
    return next;
}
export function ensureLine(flow, nodeId, lineId) {
    const node = flow.nodes.find((item) => item.id === nodeId);
    if (node && !node.lines.includes(lineId))
        node.lines = [...node.lines, lineId];
}
export function adjacentLine(source, target) {
    const common = source.lines.find((id) => target.lines.includes(id));
    return common ?? source.lines[0] ?? target.lines[0] ?? 's1';
}
function pathExists(flow, from, to) {
    if (from === to)
        return true;
    const seen = new Set();
    const stack = [from];
    while (stack.length) {
        const id = stack.pop();
        if (!id || seen.has(id))
            continue;
        seen.add(id);
        for (const edge of flow.edges) {
            if (edge.source !== id)
                continue;
            if (edge.target === to)
                return true;
            stack.push(edge.target);
        }
    }
    return false;
}
export function canConnect(flow, sourceId, targetId) {
    const source = flow.nodes.find((node) => node.id === sourceId);
    const target = flow.nodes.find((node) => node.id === targetId);
    if (!source || !target)
        return { ok: false, reason: '找不到这两个站。' };
    if (sourceId === targetId)
        return { ok: false, reason: '一站不能连到自己。' };
    if (source.kind === 'end')
        return { ok: false, reason: '终点后面不能再接站。' };
    if (target.kind === 'start')
        return { ok: false, reason: '起点前面不能再接站。' };
    if (flow.edges.some((edge) => edge.source === sourceId && edge.target === targetId)) {
        return { ok: false, reason: '这两个站已经连过了。' };
    }
    if (source.kind === 'start' && outgoing(flow, sourceId).length > 0) {
        return { ok: false, reason: '起点只连一条线出去。' };
    }
    if (target.kind === 'end' && incoming(flow, targetId).length > 0) {
        return { ok: false, reason: '终点只接一条线进来；要合流先放一个汇合。' };
    }
    if (source.kind === 'pool' && outgoing(flow, sourceId).length > 0) {
        return { ok: false, reason: '池只设一个出口。' };
    }
    if (target.kind === 'pool' && incoming(flow, targetId).length > 0) {
        return { ok: false, reason: '池只设一个入口。' };
    }
    if (source.kind === 'pool' && !isStationLike(target.kind)) {
        return { ok: false, reason: '池只能放在两站之间：上游放、下游取。' };
    }
    if (target.kind === 'pool' && !isStationLike(source.kind)) {
        return { ok: false, reason: '池只能放在两站之间：上游放、下游取。' };
    }
    if (target.kind === 'pool' && source.kind === 'pool') {
        return { ok: false, reason: '池不能直接接池。' };
    }
    if (pathExists(flow, targetId, sourceId)) {
        return { ok: false, reason: '除了「循环到满意」的站，不能成环。' };
    }
    return { ok: true };
}
export function connectNodes(flow, sourceId, targetId) {
    const next = cloneFlow(flow);
    const source = next.nodes.find((node) => node.id === sourceId);
    const target = next.nodes.find((node) => node.id === targetId);
    if (!source || !target)
        return next;
    const lineId = adjacentLine(source, target);
    next.edges.push({ id: nextId('e'), source: sourceId, target: targetId, lineId });
    ensureLine(next, sourceId, lineId);
    ensureLine(next, targetId, lineId);
    return next;
}
function reachableFrom(flow, starts) {
    const out = new Set(starts);
    const stack = [...starts];
    while (stack.length) {
        const id = stack.pop();
        if (!id)
            continue;
        for (const edge of flow.edges) {
            if (edge.source !== id)
                continue;
            if (!out.has(edge.target)) {
                out.add(edge.target);
                stack.push(edge.target);
            }
        }
    }
    return out;
}
function forwardSet(flow, from) {
    const out = new Set();
    const stack = [from];
    while (stack.length) {
        const id = stack.pop();
        if (!id || out.has(id))
            continue;
        out.add(id);
        for (const edge of flow.edges) {
            if (edge.source === id)
                stack.push(edge.target);
        }
    }
    return out;
}
function canReachSet(flow, ends) {
    const out = new Set(ends);
    const stack = [...ends];
    while (stack.length) {
        const id = stack.pop();
        if (!id)
            continue;
        for (const edge of flow.edges) {
            if (edge.target !== id)
                continue;
            if (!out.has(edge.source)) {
                out.add(edge.source);
                stack.push(edge.source);
            }
        }
    }
    return out;
}
function nextHandoff(flow, node) {
    const edge = outgoing(flow, node.id)[0];
    if (!edge)
        return '';
    const target = flow.nodes.find((item) => item.id === edge.target);
    if (!target || target.kind === 'end')
        return '';
    return (target.recv ?? '').trim();
}
function removePoolBypass(flow, nodeId) {
    const inEdges = incoming(flow, nodeId);
    const outEdges = outgoing(flow, nodeId);
    flow.edges = flow.edges.filter((edge) => edge.source !== nodeId && edge.target !== nodeId);
    flow.nodes = flow.nodes.filter((node) => node.id !== nodeId);
    if (inEdges[0] && outEdges[0]) {
        flow.edges.push({
            id: nextId('e'),
            source: inEdges[0].source,
            target: outEdges[0].target,
            lineId: inEdges[0].lineId,
        });
    }
}
function addJoinForFork(flow, forkId) {
    const fork = flow.nodes.find((node) => node.id === forkId);
    if (!fork)
        return;
    const branchEdges = outgoing(flow, forkId);
    if (branchEdges.length < 2)
        return;
    const reachSets = branchEdges.map((edge) => forwardSet(flow, edge.target));
    const ends = flow.nodes.filter((node) => node.kind === 'end' && reachSets.every((set) => set.has(node.id)));
    const end = ends[0];
    if (!end) {
        // 没有共同终点时，保守地收回多余分支，至少让检查可继续推进。
        const keep = branchEdges[0];
        flow.edges = flow.edges.filter((edge) => edge.source !== forkId || edge.id === keep.id);
        return;
    }
    const incomingToEnd = incoming(flow, end.id);
    const lineId = incomingToEnd[0]?.lineId ?? end.lines[0] ?? 's1';
    const joinNode = {
        id: nextId('n'),
        kind: 'join',
        name: '汇合',
        lines: [...new Set(incomingToEnd.flatMap((edge) => [edge.lineId, ...end.lines]))],
        position: { x: snapToGrid(end.position.x - 108), y: snapToGrid(end.position.y + 96) },
        join: 'all',
        recv: '',
        give: '',
    };
    flow.nodes.push(joinNode);
    const preds = incomingToEnd.map((edge) => ({ source: edge.source, lineId: edge.lineId }));
    flow.edges = flow.edges.filter((edge) => edge.target !== end.id);
    for (const pred of preds) {
        flow.edges.push({ id: nextId('e'), source: pred.source, target: joinNode.id, lineId: pred.lineId });
        ensureLine(flow, pred.source, pred.lineId);
    }
    flow.edges.push({ id: nextId('e'), source: joinNode.id, target: end.id, lineId });
    ensureLine(flow, end.id, lineId);
}
function connectToEnd(flow, nodeId) {
    const node = flow.nodes.find((item) => item.id === nodeId);
    if (!node)
        return;
    let ends = flow.nodes.filter((item) => item.kind === 'end');
    if (!ends.length) {
        const endNode = {
            id: nextId('n'),
            kind: 'end',
            name: '终点',
            lines: [node.lines[0] ?? 's1'],
            position: { x: snapToGrid(node.position.x + 108), y: snapToGrid(node.position.y) },
        };
        flow.nodes.push(endNode);
        ends = [endNode];
    }
    const target = ends[0];
    const reachable = forwardSet(flow, nodeId);
    const leaves = flow.nodes.filter((item) => reachable.has(item.id) && item.kind !== 'end' && outgoing(flow, item.id).length === 0);
    const tail = leaves[0] ?? node;
    const lineId = tail.lines[0] ?? flow.lines[0]?.id ?? 's1';
    flow.edges.push({ id: nextId('e'), source: tail.id, target: target.id, lineId });
    ensureLine(flow, target.id, lineId);
}
export function validateFlow(flow) {
    const errors = [];
    const infos = [];
    const nodes = nodeMap(flow);
    const starts = flow.nodes.filter((node) => node.kind === 'start').map((node) => node.id);
    const ends = flow.nodes.filter((node) => node.kind === 'end').map((node) => node.id);
    const reachable = reachableFrom(flow, starts);
    const toEnd = canReachSet(flow, ends);
    const seen = new Set();
    const add = (issue) => {
        if (seen.has(issue.id))
            return;
        seen.add(issue.id);
        if (issue.kind === 'info')
            infos.push(issue);
        else
            errors.push(issue);
    };
    for (const node of flow.nodes) {
        if (node.auto) {
            add({
                id: 'info:' + node.id,
                ref: node.id,
                message: node.auto,
                fixLabel: '知道了',
                kind: 'info',
                fix: (target) => {
                    const found = target.nodes.find((item) => item.id === node.id);
                    if (found)
                        delete found.auto;
                },
            });
        }
        if (node.kind !== 'start' && node.kind !== 'end') {
            if (!reachable.has(node.id)) {
                add({
                    id: 'connect:' + node.id,
                    ref: node.id,
                    message: '「' + node.name + '」没连上',
                    fixLabel: '删掉这个没连上的站',
                    kind: 'error',
                    fix: (target) => {
                        const index = target.nodes.findIndex((item) => item.id === node.id);
                        if (index >= 0) {
                            target.nodes.splice(index, 1);
                            target.edges = target.edges.filter((edge) => edge.source !== node.id && edge.target !== node.id);
                        }
                    },
                });
            }
            else if (!toEnd.has(node.id)) {
                add({
                    id: 'end:' + node.id,
                    ref: node.id,
                    message: '「' + node.name + '」走不到终点',
                    fixLabel: '接到终点',
                    kind: 'error',
                    fix: (target) => connectToEnd(target, node.id),
                });
            }
        }
        if (node.kind === 'pool') {
            const inEdges = incoming(flow, node.id);
            const outEdges = outgoing(flow, node.id);
            const source = inEdges[0] ? nodes.get(inEdges[0].source) : undefined;
            const target = outEdges[0] ? nodes.get(outEdges[0].target) : undefined;
            const bad = inEdges.length !== 1 ||
                outEdges.length !== 1 ||
                !source ||
                !target ||
                !isStationLike(source.kind) ||
                !isStationLike(target.kind);
            if (bad) {
                add({
                    id: 'pool:' + node.id,
                    ref: node.id,
                    message: '池要夹在主线的两站之间：上游放、下游取',
                    fixLabel: '去掉这个池',
                    kind: 'error',
                    fix: (targetFlow) => removePoolBypass(targetFlow, node.id),
                });
            }
            continue;
        }
        if (node.kind === 'gate') {
            if (!(node.members ?? []).length) {
                add({
                    id: 'members:' + node.id,
                    ref: node.id,
                    message: '「' + node.name + '」里还没有人',
                    fixLabel: '加一位审读',
                    kind: 'error',
                    fix: (target) => {
                        const found = target.nodes.find((item) => item.id === node.id);
                        if (found)
                            found.members = ['审读'];
                    },
                });
            }
        }
        else if ((node.kind === 'station' || node.kind === 'stack') && !(node.who ?? []).length) {
            add({
                id: 'who:' + node.id,
                ref: node.id,
                message: '「' + node.name + '」还没定谁来做',
                fixLabel: '先交给 Kind',
                kind: 'error',
                fix: (target) => {
                    const found = target.nodes.find((item) => item.id === node.id);
                    if (found)
                        found.who = ['Kind'];
                },
            });
        }
        if (node.kind === 'station' || node.kind === 'stack' || node.kind === 'gate') {
            if (!(node.give ?? '').trim()) {
                const suggested = nextHandoff(flow, node) || node.name + '的结果';
                add({
                    id: 'give:' + node.id,
                    ref: node.id,
                    message: '「' + node.name + '」没写交出什么',
                    fixLabel: '补成「' + suggested + '」',
                    kind: 'error',
                    fix: (target) => {
                        const found = target.nodes.find((item) => item.id === node.id);
                        if (found)
                            found.give = suggested;
                    },
                });
            }
        }
        if (node.ext && !node.user) {
            add({
                id: 'ext:' + node.id,
                ref: node.id,
                message: '「' + node.name + '」对外发出去收不回，要每次等你',
                fixLabel: '改成每次等你',
                kind: 'error',
                fix: (target) => {
                    const found = target.nodes.find((item) => item.id === node.id);
                    if (found)
                        found.user = true;
                },
            });
        }
        if (node.loop && (!node.loop.max || node.loop.max < 1)) {
            add({
                id: 'loop:' + node.id,
                ref: node.id,
                message: '「' + node.name + '」的循环没写上限',
                fixLabel: '上限 3 次',
                kind: 'error',
                fix: (target) => {
                    const found = target.nodes.find((item) => item.id === node.id);
                    if (found && found.loop)
                        found.loop.max = 3;
                },
            });
        }
        if (node.kind === 'join' && !node.join) {
            add({
                id: 'join:' + node.id,
                ref: node.id,
                message: '「' + node.name + '」还没写怎么汇合',
                fixLabel: '设为「全部到齐」',
                kind: 'error',
                fix: (target) => {
                    const found = target.nodes.find((item) => item.id === node.id);
                    if (found)
                        found.join = 'all';
                },
            });
        }
        if (node.kind !== 'start' && node.kind !== 'end') {
            const outs = outgoing(flow, node.id);
            if (outs.length > 1) {
                const reachSets = outs.map((edge) => forwardSet(flow, edge.target));
                const joins = flow.nodes.filter((item) => item.kind === 'join' && reachSets.every((set) => set.has(item.id)));
                if (!joins.length) {
                    add({
                        id: 'fork:' + node.id,
                        ref: node.id,
                        message: '「' + node.name + '」分出去没有汇合',
                        fixLabel: '补一个汇合',
                        kind: 'error',
                        fix: (target) => addJoinForFork(target, node.id),
                    });
                }
            }
        }
    }
    // 成环：除了标了「循环到满意」的站，别的站不允许绕回来。
    const state = new Map();
    const stack = [];
    const cycleKeys = new Set();
    const visit = (nodeId) => {
        state.set(nodeId, 1);
        stack.push(nodeId);
        for (const edge of flow.edges) {
            if (edge.source !== nodeId)
                continue;
            const target = edge.target;
            const targetState = state.get(target) ?? 0;
            if (targetState === 1) {
                const at = stack.indexOf(target);
                if (at >= 0) {
                    const cycle = stack.slice(at);
                    const key = [...cycle].sort().join('|');
                    if (!cycleKeys.has(key)) {
                        cycleKeys.add(key);
                        const allLoop = cycle.every((id) => {
                            const item = nodes.get(id);
                            return item?.loop && item.loop.max && item.loop.max > 0;
                        });
                        if (!allLoop) {
                            const startNode = nodes.get(nodeId);
                            const targetNode = nodes.get(target);
                            add({
                                id: 'cycle:' + key,
                                ref: nodeId,
                                message: '「' + (startNode?.name ?? '这一站') + '」绕回了「' + (targetNode?.name ?? '前面') + '」：除了「循环到满意」的站，不能成环',
                                fixLabel: '断开这条回头的线',
                                kind: 'error',
                                fix: (targetFlow) => {
                                    targetFlow.edges = targetFlow.edges.filter((item) => item.id !== edge.id);
                                },
                            });
                        }
                    }
                }
            }
            else if (targetState === 0) {
                visit(target);
            }
        }
        stack.pop();
        state.set(nodeId, 2);
    };
    for (const node of flow.nodes)
        if ((state.get(node.id) ?? 0) === 0)
            visit(node.id);
    return { errors, infos };
}
function graphSignature(flow) {
    return JSON.stringify({ nodes: flow.nodes, edges: flow.edges });
}
export function applyAllFixes(flow) {
    const next = cloneFlow(flow);
    for (let i = 0; i < 50; i += 1) {
        const before = graphSignature(next);
        const issue = validateFlow(next).errors[0];
        if (!issue)
            break;
        issue.fix(next);
        if (graphSignature(next) === before)
            break;
    }
    return next;
}
function midpointOnEdge(flow, edge) {
    const source = flow.nodes.find((node) => node.id === edge.source);
    const target = flow.nodes.find((node) => node.id === edge.target);
    if (!source || !target)
        return { x: 0, y: 0 };
    const sx = source.position.x + NODE_W;
    const sy = source.position.y + NODE_H / 2;
    const tx = target.position.x;
    const ty = target.position.y + NODE_H / 2;
    return { x: snapToGrid((sx + tx) / 2 - NODE_W / 2), y: snapToGrid((sy + ty) / 2 - NODE_H / 2) };
}
function makeNode(partial) {
    return { id: nextId('n'), recv: '', give: '', dod: [], ...partial };
}
export function kindRespond(flow, text, selectedId) {
    if (/一天同意一次|常设同意|不用每次|一次同意/.test(text)) {
        return {
            reply: '对外发出去收不回，这类动作按平台规则每次都等你点头，这一条我不改。发布的平台、时间这些可以调。',
        };
    }
    const ops = [];
    const byName = (name) => flow.nodes.find((node) => node.name === name)?.id;
    const named = flow.nodes.filter((node) => text.includes(node.name)).map((node) => node.id);
    const selected = selectedId && flow.nodes.some((node) => node.id === selectedId) ? selectedId : null;
    for (const match of text.matchAll(/去掉「?([^，,。」\s]+)」?/g)) {
        const id = byName(match[1]);
        if (id)
            ops.push({ op: 'del', ref: id });
    }
    if (/查重/.test(text) && !ops.some((op) => op.op === 'ins')) {
        const gate = flow.nodes.find((node) => node.kind === 'gate');
        const edge = gate ? incoming(flow, gate.id)[0] : undefined;
        if (edge) {
            const node = makeNode({
                kind: 'station',
                name: '查重',
                lines: [edge.lineId],
                position: midpointOnEdge(flow, edge),
                who: ['程序'],
                skills: ['文本查重'],
                recv: '草稿',
                give: '查重报告、可疑段落',
                dod: [{ text: '和已发文章重复率低于 10%', auto: true }],
            });
            ops.push({ op: 'ins', node, edgeId: edge.id });
        }
    }
    const tm = text.match(/(\d{1,2}:\d{2})/);
    if (tm) {
        for (const id of named) {
            const node = flow.nodes.find((item) => item.id === id);
            if (node?.rhythm && /\d{1,2}:\d{2}/.test(node.rhythm)) {
                ops.push({ op: 'chg', ref: id, patch: { rhythm: node.rhythm.replace(/\d{1,2}:\d{2}/, tm[1]) } });
            }
        }
    }
    if (/安全/.test(text)) {
        for (const id of named) {
            const node = flow.nodes.find((item) => item.id === id);
            if (node?.kind === 'gate' && !(node.members ?? []).includes('安全')) {
                ops.push({
                    op: 'chg',
                    ref: id,
                    patch: { members: [...(node.members ?? []), '安全'], rule: '一票否决' },
                });
            }
        }
    }
    const lm = text.match(/循环\s*(\d+)\s*次/);
    if (lm) {
        const id = named[0] ?? selected;
        const node = id ? flow.nodes.find((item) => item.id === id) : undefined;
        if (node && node.kind !== 'pool' && node.kind !== 'join' && node.kind !== 'start' && node.kind !== 'end') {
            ops.push({ op: 'chg', ref: node.id, patch: { loop: { max: Number(lm[1]), until: '下一道门过了' } } });
        }
    }
    if (/等我|等你|确认/.test(text) && !ops.length) {
        const id = named[0] ?? selected;
        const node = id ? flow.nodes.find((item) => item.id === id) : undefined;
        if (node && !node.user)
            ops.push({ op: 'chg', ref: node.id, patch: { user: true } });
    }
    if (!ops.length && selected) {
        const node = flow.nodes.find((item) => item.id === selected);
        const afterEdge = node ? outgoing(flow, node.id)[0] : undefined;
        if (node && afterEdge) {
            const fresh = makeNode({
                kind: 'station',
                name: '复核',
                lines: [afterEdge.lineId],
                position: midpointOnEdge(flow, afterEdge),
                who: ['审读'],
                recv: node.give ?? '',
                give: '复核意见',
                dod: [{ text: '每处问题都写明位置', auto: true }],
            });
            ops.push({ op: 'ins', node: fresh, edgeId: afterEdge.id });
        }
    }
    if (!ops.length) {
        return { reply: '我没对上具体哪一站。先在图上点一站再说，或者把站名写进来。' };
    }
    return { ops };
}
function splitEdge(flow, edgeId, node, diff) {
    const inserted = insertNodeOnEdge(flow, edgeId, node);
    flow.nodes = inserted.nodes;
    flow.edges = inserted.edges;
    if (!diff)
        return;
    const found = flow.nodes.find((item) => item.id === node.id);
    if (found)
        found.diff = diff;
    for (const edge of flow.edges) {
        if (edge.source === node.id || edge.target === node.id)
            edge.diff = diff;
    }
}
export function applyOps(flow, ops, preview) {
    const next = cloneFlow(flow);
    for (const op of ops) {
        if (op.op === 'ins') {
            const node = clone(op.node);
            if (preview)
                node.diff = 'add';
            splitEdge(next, op.edgeId, node, preview ? 'add' : undefined);
        }
        else if (op.op === 'del') {
            const node = next.nodes.find((item) => item.id === op.ref);
            if (!node)
                continue;
            if (preview) {
                node.diff = 'del';
                for (const edge of next.edges) {
                    if (edge.source === node.id || edge.target === node.id)
                        edge.diff = 'del';
                }
            }
            else {
                next.nodes = next.nodes.filter((item) => item.id !== op.ref);
                next.edges = next.edges.filter((edge) => edge.source !== op.ref && edge.target !== op.ref);
            }
        }
        else {
            const node = next.nodes.find((item) => item.id === op.ref);
            if (!node)
                continue;
            const old = {};
            for (const key of Object.keys(op.patch)) {
                old[key] = clone(node[key] ?? null);
            }
            Object.assign(node, clone(op.patch));
            if (preview) {
                node.diff = 'chg';
                node.diffOld = old;
            }
        }
    }
    return next;
}
export function summarizeOps(flow, ops) {
    const nameOf = (op) => {
        if (op.op === 'ins')
            return op.node.name;
        return flow.nodes.find((node) => node.id === op.ref)?.name ?? '这一站';
    };
    const groups = [
        { key: 'ins', label: '新加' },
        { key: 'del', label: '去掉' },
        { key: 'chg', label: '改了' },
    ];
    return groups
        .map(({ key, label }) => {
        const names = ops.filter((op) => op.op === key).map(nameOf);
        return names.length ? label + '「' + names.join('」「') + '」' : '';
    })
        .filter(Boolean)
        .join('；');
}
export function countOps(ops) {
    return {
        add: ops.filter((op) => op.op === 'ins').length,
        del: ops.filter((op) => op.op === 'del').length,
        chg: ops.filter((op) => op.op === 'chg').length,
    };
}
let elkPromise = null;
async function getElk() {
    if (!elkPromise) {
        elkPromise = import('elkjs/lib/elk.bundled.js').then((module) => new module.default());
    }
    return elkPromise;
}
export async function layoutFlow(flow) {
    const ids = new Set(flow.nodes.map((node) => node.id));
    const graph = {
        id: 'root',
        layoutOptions: {
            'elk.algorithm': 'layered',
            'elk.direction': 'RIGHT',
            'elk.layered.spacing.nodeNodeBetweenLayers': '48',
            'elk.spacing.nodeNode': '30',
            'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF',
            'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
            'elk.edgeRouting': 'ORTHOGONAL',
            'elk.padding': '[top=32,left=16,bottom=32,right=16]',
        },
        children: flow.nodes.map((node) => ({
            id: node.id,
            width: NODE_W,
            height: NODE_H,
        })),
        edges: flow.edges
            .filter((edge) => ids.has(edge.source) && ids.has(edge.target))
            .map((edge) => ({ id: edge.id, sources: [edge.source], targets: [edge.target] })),
    };
    const engine = await getElk();
    const result = await engine.layout(graph);
    const positions = {};
    for (const child of result.children ?? []) {
        positions[child.id] = {
            x: snapToGrid(child.x ?? 0),
            y: snapToGrid(child.y ?? 0),
        };
    }
    return positions;
}
/* ───────────────────────── 位置存取 ───────────────────────── */
const STORAGE_PREFIX = 'aios-v14-flow-pos:';
export function positionsKey(flowId) {
    return STORAGE_PREFIX + flowId;
}
export function readPositions(flowId) {
    if (typeof window === 'undefined')
        return null;
    try {
        const raw = window.localStorage.getItem(positionsKey(flowId));
        if (!raw)
            return null;
        const parsed = JSON.parse(raw);
        if (!parsed || typeof parsed !== 'object')
            return null;
        const out = {};
        for (const [id, value] of Object.entries(parsed)) {
            if (!value || typeof value !== 'object')
                continue;
            const point = value;
            if (typeof point.x !== 'number' || typeof point.y !== 'number')
                continue;
            if (!Number.isFinite(point.x) || !Number.isFinite(point.y))
                continue;
            out[id] = { x: point.x, y: point.y };
        }
        return Object.keys(out).length ? out : null;
    }
    catch {
        return null;
    }
}
export function writePositions(flowId, nodes) {
    if (typeof window === 'undefined')
        return;
    try {
        const data = {};
        for (const node of nodes)
            data[node.id] = { x: node.position.x, y: node.position.y };
        window.localStorage.setItem(positionsKey(flowId), JSON.stringify(data));
    }
    catch {
        // 原型：存不进去就算了，下次打开用整理后的位置。
    }
}
/** 线的画法：从站点中心到站点中心；同一个高度就走直线。 */
export function flowEdgePath(sx, sy, tx, ty) {
    if (Math.abs(ty - sy) < 1)
        return 'M ' + sx + ' ' + sy + ' L ' + tx + ' ' + ty;
    const midX = (sx + tx) / 2;
    const dir = Math.sign(ty - sy) || 1;
    const r = Math.max(2, Math.min(14, Math.abs(ty - sy) / 2, Math.abs(tx - sx) / 2));
    const hEnd = tx >= sx ? midX - r : midX + r;
    const hStart = tx >= sx ? midX + r : midX - r;
    return ('M ' + sx + ' ' + sy +
        ' H ' + hEnd +
        ' Q ' + midX + ' ' + sy + ' ' + midX + ' ' + (sy + dir * r) +
        ' V ' + (ty - dir * r) +
        ' Q ' + midX + ' ' + ty + ' ' + hStart + ' ' + ty +
        ' H ' + tx);
}
export function edgeLines(flow) {
    const map = nodeMap(flow);
    const list = [];
    for (const edge of flow.edges) {
        const source = map.get(edge.source);
        const target = map.get(edge.target);
        if (!source || !target)
            continue;
        const lineIndex = Math.max(0, flow.lines.findIndex((line) => line.id === edge.lineId));
        const laneOffset = flow.lines.length > 1 ? (lineIndex - (flow.lines.length - 1) / 2) * 6 : 0;
        const sx = source.position.x + NODE_W / 2;
        const sy = source.position.y + NODE_H / 2 + laneOffset;
        const tx = target.position.x + NODE_W / 2;
        const ty = target.position.y + NODE_H / 2 + laneOffset;
        const points = Math.abs(sy - ty) < 1
            ? [{ x: sx, y: sy }, { x: tx, y: ty }]
            : [
                { x: sx, y: sy },
                { x: (sx + tx) / 2, y: sy },
                { x: (sx + tx) / 2, y: ty },
                { x: tx, y: ty },
            ];
        list.push({ edge, points });
    }
    return list;
}
function pointSegmentDistance(point, a, b) {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    if (dx === 0 && dy === 0)
        return Math.hypot(point.x - a.x, point.y - a.y);
    const t = Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / (dx * dx + dy * dy)));
    return Math.hypot(point.x - (a.x + t * dx), point.y - (a.y + t * dy));
}
export function distanceToEdge(point, line) {
    let best = Number.POSITIVE_INFINITY;
    for (let i = 0; i < line.points.length - 1; i += 1) {
        best = Math.min(best, pointSegmentDistance(point, line.points[i], line.points[i + 1]));
    }
    return best;
}
export function projectOnEdge(point, line) {
    let best = line.points[0];
    let bestDistance = Number.POSITIVE_INFINITY;
    for (let i = 0; i < line.points.length - 1; i += 1) {
        const a = line.points[i];
        const b = line.points[i + 1];
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const length = dx * dx + dy * dy;
        const t = length === 0 ? 0 : Math.max(0, Math.min(1, ((point.x - a.x) * dx + (point.y - a.y) * dy) / length));
        const candidate = { x: a.x + t * dx, y: a.y + t * dy };
        const distance = Math.hypot(point.x - candidate.x, point.y - candidate.y);
        if (distance < bestDistance) {
            bestDistance = distance;
            best = candidate;
        }
    }
    return best;
}
export function canDropOnEdge(flow, part, edgeId) {
    if (part === 'join')
        return { ok: false, reason: '汇合拖到空白处，再把几支连进来' };
    const edge = flow.edges.find((item) => item.id === edgeId);
    if (!edge)
        return { ok: false, reason: '没对准线' };
    const source = flow.nodes.find((node) => node.id === edge.source);
    const target = flow.nodes.find((node) => node.id === edge.target);
    if (part === 'pool') {
        if (!source || !target || !isStationLike(source.kind) || !isStationLike(target.kind)) {
            return { ok: false, reason: '池只能放在主线上、两站之间' };
        }
    }
    return { ok: true };
}
export function partNodeKind(part) {
    if (part === 'user')
        return 'station';
    if (part === 'stack')
        return 'stack';
    if (part === 'gate')
        return 'gate';
    if (part === 'pool')
        return 'pool';
    if (part === 'join')
        return 'join';
    return 'station';
}
export function makeDefaultDod() {
    return [];
}
export function joinLabel(mode) {
    return mode ? JOIN_LABEL[mode] : '怎么汇合？';
}
export function nonTerminal(node) {
    return !TERMINAL_LIKE.has(node.kind);
}
