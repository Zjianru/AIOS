/**
 * 用固定种子生成两条长记录：
 *   - Kind 的主对话：2000 轮；
 *   - 一件任务的运行记录：520 轮（第 2 期的任务详情用）。
 *
 * 生成方式是「按 index 算」，不是「一次算完」：第 i 轮永远由 (SEED, i) 决定，
 * 所以 2000 轮的体积不进内存、不受加载顺序影响，翻到哪页算哪页。
 * 同一个 index 每次算出来一模一样（需求 §二）。
 */
import { ARTIFACT_NAMES, APPROVAL_COMMANDS, CODE_BLOCK, DECISION_OPTIONS, LONG_MD, MARKDOWN_TABLE, MEMORY_NOTES, PROBLEMS, SHORT_ANSWER, THINKING_FULL, THINKING_SUMMARY, TOOL_CALLS, TOOL_STEPS, USER_SAY, WAKE_TEXTS, md, } from './content';
import { artifacts, kindLog, projects, rootTaskIds, tasks } from './fixtures/workspace';
import { hashSeed, intBetween, mulberry32, pick, pickN, seededAt } from './rng';
export const KIND_THREAD_ID = 'thread_kind_main';
export const KIND_TURN_COUNT = 2000;
export const TASK_RUN_THREAD_ID = 'thread_run_root';
export const TASK_RUN_TURN_COUNT = 520;
const KIND_SEED = hashSeed('aios-v14/kind/2000');
const RUN_SEED = hashSeed('aios-v14/task-run/520');
/** 固定基准时间：2026-10-27 14:05（+08:00）。时间戳固定，截图和测速才可重复。 */
const KIND_BASE_TS = Math.floor(Date.UTC(2026, 9, 27, 6, 5, 0) / 1000);
const RUN_BASE_TS = Math.floor(Date.UTC(2026, 9, 27, 4, 20, 0) / 1000);
/* ───────────────────────── 时间轴 ───────────────────────── */
function buildStarts(count, seed, baseTs, longGapChance) {
    const rnd = mulberry32(seed ^ 0x9e3779b9);
    const out = new Array(count);
    let t = baseTs;
    out[count - 1] = t;
    for (let i = count - 2; i >= 0; i--) {
        const r = rnd();
        const gap = r < longGapChance ? intBetween(rnd, 3 * 3600, 9 * 3600)
            : r < longGapChance + 0.44 ? intBetween(rnd, 15, 180)
                : intBetween(rnd, 180, 1800);
        t -= gap;
        out[i] = t;
    }
    return out;
}
const kindStarts = buildStarts(KIND_TURN_COUNT, KIND_SEED, KIND_BASE_TS, 0.06);
const runStarts = buildStarts(TASK_RUN_TURN_COUNT, RUN_SEED, RUN_BASE_TS, 0.02);
const QUOTES = [
    { kind: 'role', word: '@研究员', label: '@研究员', target: '#team' },
    { kind: 'role', word: '@评测员', label: '@评测员', target: '#team' },
    { kind: 'capability', word: '$网页搜索', label: '$网页搜索', target: '#ability' },
    { kind: 'capability', word: '$命令执行', label: '$命令执行', target: '#ability' },
    { kind: 'material', word: '▤ 评测方法 › 免费额度', label: '▤ 搜索引擎评测方法 › §2 成本', target: '#read:搜索引擎评测方法.pdf' },
    { kind: 'material', word: '▤ 我的简介 › 读者是谁', label: '▤ 我的简介.md › 读者是谁', target: '#read:我的简介.md' },
    { kind: 'artifact', word: '▤ 评测记录 v2', label: '▤ 评测记录.md v2', target: '#read:评测记录.md' },
    { kind: 'task', word: '◐ 多引擎实测', label: '◐ 多引擎实测', target: '#task:B2' },
    { kind: 'task', word: '◐ 可行性验证', label: '◐ 可行性验证', target: '#task:C2' },
    { kind: 'project', word: '■ web-search-pro 研究', label: '■ web-search-pro 研究', target: '#project:wsp' },
    { kind: 'project', word: '■ 日常', label: '■ 日常', target: '#project:daily' },
    { kind: 'pin', word: '📍1 外包协议草案 › 第 3 条', label: '📍1 外包协议草案 › 第 3 条', target: '#read:外包协议草案.docx' },
];
/** 在一句话末尾挂一个引用片，并把 span 记进 textElements。 */
function decorate(rnd, base) {
    if (rnd() > 0.28)
        return { text: base, elements: [] };
    const q = pick(rnd, QUOTES);
    const text = rnd() > 0.5 ? base + ' ' + q.word : q.word + ' ' + base;
    const start = text.indexOf(q.word);
    return { text, elements: [{ start, end: start + q.word.length, kind: q.kind, label: q.label, target: q.target }] };
}
/* ───────────────────────── 卡片（v10-tables §2.2 封闭类型表） ───────────────────────── */
const originPool = [
    { projectCode: 'W', projectName: projects.wsp.name, taskTitle: '多引擎实测', go: '#task:B2' },
    { projectCode: 'W', projectName: projects.wsp.name, taskTitle: '可行性验证', go: '#task:C2' },
    { projectCode: 'N', projectName: projects.news.name, taskTitle: '写说明', go: '#task:N3' },
    { projectCode: '日', projectName: projects.daily.name, taskTitle: '核实来源', go: '#task:D2' },
    { projectCode: null, projectName: null, taskTitle: null, go: null },
];
function timeLabel(ts) {
    const d = new Date(ts * 1000);
    const hh = String(d.getHours()).padStart(2, '0');
    const mm = String(d.getMinutes()).padStart(2, '0');
    return hh + ':' + mm;
}
function makeCard(rnd, seedIndex, ts) {
    const typeIdx = Math.floor(rnd() * 14);
    const origin = pick(rnd, originPool);
    const id = 'card_' + seedIndex + '_' + Math.floor(rnd() * 1e6).toString(36);
    const time = timeLabel(ts + intBetween(rnd, 3, 60));
    const base = { id, origin, time, done: null };
    const art = pick(rnd, ARTIFACT_NAMES);
    switch (typeIdx) {
        case 0:
            return { ...base, type: 'routeDraft', title: '路线草稿：一件事拆成四站',
                body: { kind: 'routeDraft',
                    stations: [
                        { role: '研究员', title: '竞争景观', what: '整理 3 类替代方案' },
                        { role: '评测员', title: '实证评测', what: '4 个引擎各跑 20 个查询' },
                        { role: '研究员', title: '可能性探索', what: '验证 4 个场景是否可行' },
                        { role: 'Kind', title: '价值判断', what: '给出继续 / 改造 / 放弃' },
                    ],
                    criterion: '覆盖 3 类以上替代方案；至少 3 个引擎的实测；给出判断与理由。',
                    estimate: '≈ 1 小时 20 分' },
                actions: [{ label: '开工', primary: true, mutates: true }, { label: '改一下' }] };
        case 1:
            return { ...base, type: 'task', title: '多引擎实测正在跑',
                body: { kind: 'task', progress: '3 / 4 引擎', running: 'Brave 的第 12 个查询' },
                actions: [{ label: '去看 ›' }] };
        case 2:
            return { ...base, type: 'askDecision', title: 'Bing 缺 key，第 4 个引擎要不要补测',
                body: { kind: 'askDecision', options: pick(rnd, DECISION_OPTIONS).map(o => ({ ...o })) },
                actions: [] };
        case 3: {
            const command = pick(rnd, APPROVAL_COMMANDS);
            // 不可逆的动作只给「允许这一次 / 不允许」；其余可以「加进范围」（D-27 晚 7 处冲突第 1 条）。
            const irreversible = command.includes('rm -rf');
            return {
                ...base,
                type: 'askApproval',
                title: irreversible ? '要删掉引擎缓存目录（删了收不回）' : '要跑一条会写盘的命令',
                body: { kind: 'askApproval', command, why: irreversible ? '缓存里是上一轮的原始结果，删掉后要重跑一遍。' : '这条命令会写盘，按规矩先问你。' },
                actions: irreversible
                    ? [{ label: '允许这一次', primary: true, mutates: true }, { label: '不允许' }]
                    : [{ label: '允许这一次', primary: true, mutates: true }, { label: '加进范围', mutates: true }, { label: '不允许' }],
            };
        }
        case 4:
            return { ...base, type: 'askWeb', title: '要你在网页上完成登录授权',
                body: { kind: 'askWeb', url: 'https://brave.com/search/api/', why: '需要你登录后把 key 贴回来，我读不到你的账号。' },
                actions: [{ label: '去完成', primary: true }, { label: '不授权' }] };
        case 5:
            return { ...base, type: 'artifact', title: art + ' v2 写好了',
                body: { kind: 'artifact', artifact: art, version: 'v2',
                    changes: ['去重后命中率从 21 / 20 修正为 18 / 20', '结论里注明缺 Bing 这一项'],
                    satisfied: [{ ok: true, text: '同一组 20 个查询，测 4 个引擎' }, { ok: false, text: '命中率和耗时按 §3 计算' }] },
                actions: [{ label: '验收', primary: true, mutates: true }, { label: '打开' }] };
        case 6: {
            const m = pick(rnd, MEMORY_NOTES);
            return { ...base, type: 'memory', title: '记下一条', body: { kind: 'memory', note: m.note, noteKind: m.noteKind },
                actions: [{ label: '更正' }, { label: '忘记', mutates: true }] };
        }
        case 7: {
            const w = pick(rnd, WAKE_TEXTS);
            return { ...base, type: 'wake', title: '到点提醒你', body: { kind: 'wake', when: w.when, todo: w.todo },
                actions: [{ label: '改时间' }, { label: '取消', mutates: true }] };
        }
        case 8:
            return { ...base, type: 'roleJoin', title: '研究员加入了这件事',
                body: { kind: 'roleJoin', role: '研究员', why: '这件事要读一手来源并做对比，研究员的经验刚好对得上。' },
                actions: [{ label: '让他离开', mutates: true }] };
        case 9:
            return { ...base, type: 'planChanged', title: '计划变了：多引擎实测挪到并行',
                body: { kind: 'planChanged',
                    before: ['竞争景观 → 实证评测 → 可能性探索'],
                    after: ['竞争景观 ∥ 实证评测 → 可能性探索'],
                    why: '两件事没有依赖，串行做多花 20 分钟。' },
                actions: [{ label: '知道了', primary: true }, { label: '撤回这次改动', mutates: true }] };
        case 10:
            return { ...base, type: 'milestone', title: '里程碑「读懂仓库」4 条标准，3 条满足',
                body: { kind: 'milestone', criteria: [
                        { ok: true, text: '说明技术栈与模块划分' },
                        { ok: true, text: '写清楚怎么运行' },
                        { ok: true, text: '评估完成度' },
                        { ok: false, text: '列出可改进点并排序（只列了 4 条，没有排序）' },
                    ] },
                actions: [{ label: '通过', primary: true, mutates: true }, { label: '不通过' }] };
        case 11:
            return { ...base, type: 'assignRole', title: '推荐用评测员做这一站',
                body: { kind: 'assignRole', role: '评测员', why: '这一站要真的把东西跑起来，评测员手上有一套现成的实测流程。' },
                actions: [{ label: '确认', primary: true, mutates: true }, { label: '换人' }] };
        case 12:
            return { ...base, type: 'quotaAlert', title: '提醒线到了：这件事已用 68%',
                body: { kind: 'quotaAlert', line: 'web-search-pro 研究 · 提醒线 70%', used: '68% · 约 6.8k tokens' },
                actions: [{ label: '继续' }, { label: '暂停这件事', mutates: true }, { label: '调整提醒线' }] };
        default: {
            const p = pick(rnd, PROBLEMS);
            return { ...base, type: 'problem', title: '出了问题：' + p.what, body: { kind: 'problem', what: p.what, handled: p.handled },
                actions: [{ label: '去看 ›' }] };
        }
    }
}
const SHAPES = [
    ['quiet', 0.2],
    ['short', 0.16],
    ['long', 0.1],
    ['table', 0.08],
    ['code', 0.07],
    ['tool', 0.13],
    ['cards', 0.16],
    ['mixed', 0.1],
];
function pickShape(rnd) {
    let r = rnd();
    for (const [shape, w] of SHAPES) {
        if (r < w)
            return shape;
        r -= w;
    }
    return 'quiet';
}
function itemsForShape(rnd, index, ts, shape) {
    const items = [];
    const say = decorate(rnd, pick(rnd, USER_SAY));
    items.push({
        type: 'userMessage',
        id: 'u_' + index,
        clientId: null,
        content: [{ type: 'text', text: say.text, textElements: say.elements }],
    });
    const withThinking = shape !== 'quiet' && rnd() < 0.55;
    if (withThinking) {
        items.push({
            type: 'reasoning',
            id: 'r_' + index,
            summary: pickN(rnd, THINKING_SUMMARY, intBetween(rnd, 1, 3)),
            content: [pick(rnd, THINKING_FULL)],
        });
    }
    let answer = pick(rnd, SHORT_ANSWER);
    if (shape === 'long')
        answer = md(pick(rnd, LONG_MD));
    if (shape === 'table')
        answer = '整理好了，直接看表：' + String.fromCharCode(10) + String.fromCharCode(10) + md(pick(rnd, MARKDOWN_TABLE));
    if (shape === 'code')
        answer = md(pick(rnd, CODE_BLOCK));
    if (shape === 'mixed')
        answer = md(pick(rnd, LONG_MD)) + String.fromCharCode(10) + String.fromCharCode(10) + md(pick(rnd, MARKDOWN_TABLE));
    const toolCount = shape === 'tool' ? intBetween(rnd, 1, 2) : shape === 'mixed' ? 1 : 0;
    for (let k = 0; k < toolCount; k++) {
        const step = pick(rnd, TOOL_STEPS);
        items.push({
            type: 'commandExecution',
            id: 'x_' + index + '_' + k,
            command: step.command,
            cwd: step.cwd,
            status: 'completed',
            aggregatedOutput: step.out,
            exitCode: step.exit,
            durationMs: step.ms,
        });
        const [name, arg] = pick(rnd, TOOL_CALLS);
        items.push({ type: 'mcpToolCall', id: 'm_' + index + '_' + k, server: 'aios', tool: name, status: 'completed', arguments: { arg }, durationMs: intBetween(rnd, 60, 1800) });
    }
    const cardCount = shape === 'cards' ? intBetween(rnd, 1, 3) : shape === 'quiet' ? 0 : rnd() < 0.22 ? 1 : 0;
    for (let k = 0; k < cardCount; k++) {
        items.push({ type: 'agentCard', id: 'c_' + index + '_' + k, card: makeCard(rnd, index * 7 + k, ts) });
    }
    if (shape !== 'quiet' || rnd() < 0.5) {
        items.push({ type: 'agentMessage', id: 'a_' + index, text: answer, phase: 'final_answer' });
    }
    return items;
}
function turnStatus(index, total) {
    if (index === total - 1)
        return 'inProgress';
    if (index % 617 === 0)
        return 'interrupted';
    if (index % 431 === 0)
        return 'failed';
    return 'completed';
}
export function kindTurnAt(index) {
    const rnd = seededAt(KIND_SEED, index);
    const ts = kindStarts[index];
    const shape = pickShape(rnd);
    const items = itemsForShape(rnd, index, ts, shape);
    const status = turnStatus(index, KIND_TURN_COUNT);
    const dur = intBetween(rnd, 1200, 48000);
    return {
        id: 'turn_' + String(index).padStart(4, '0'),
        items,
        itemsView: 'full',
        status,
        error: status === 'failed' ? { message: '模型调用超时（原型里是写好的样例）' } : null,
        startedAt: ts,
        completedAt: status === 'inProgress' ? null : ts + Math.round(dur / 1000),
        durationMs: status === 'inProgress' ? null : dur,
    };
}
const kindThread = {
    id: KIND_THREAD_ID,
    preview: 'web-search-pro 还有没有价值；news-intelligence 结构说明；每日简报',
    model: 'DeepSeek V4 Pro',
    modelProvider: 'deepseek',
    cwd: '~/projects',
    createdAt: kindStarts[0],
    updatedAt: kindStarts[KIND_TURN_COUNT - 1],
    status: 'inProgress',
};
export const kindSource = {
    thread: kindThread,
    totalTurns: KIND_TURN_COUNT,
    startedAt: (i) => kindStarts[i],
    turnAt: kindTurnAt,
};
/* ───────────────────────── 一件任务的运行记录（第 2 期用） ───────────────────────── */
const RUN_EVENTS = [
    ['评测员', '模型请求', 'MiniMax M3.1 Flash · 输入 6.2k / 输出 1.1k'],
    ['engine-runner-1', '工具调用', 'web_search × 20 · 全部返回'],
    ['engine-runner-2', '工具调用', 'web_search × 20 · 2 次超时后重试成功'],
    ['评测员', '工具调用', 'web_search × 20（Brave）'],
    ['评测员', '文件修改', 'scripts/dedupe.py（+18）'],
    ['评测员', '提交候选', '评测记录.md · 候选 c2'],
    ['审查员', '审查结论', '需修正：结果未去重'],
    ['审查员', '审查结论', '通过'],
    ['Kind', '交回成果', '评测记录.md · c2'],
    ['Kind', '检查点', '上下文压缩成一张检查点卡'],
];
export function runTurnAt(index) {
    const rnd = seededAt(RUN_SEED, index);
    const ts = runStarts[index];
    const [role, kind, what] = pick(rnd, RUN_EVENTS);
    const items = [
        { type: 'agentMessage', id: 'ra_' + index, text: role + ' · ' + kind + ' · ' + what },
    ];
    if (rnd() < 0.35) {
        const step = pick(rnd, TOOL_STEPS);
        items.push({ type: 'commandExecution', id: 'rx_' + index, command: step.command, cwd: step.cwd, status: 'completed', aggregatedOutput: step.out, exitCode: step.exit, durationMs: step.ms });
    }
    const status = index === TASK_RUN_TURN_COUNT - 1 ? 'inProgress' : 'completed';
    return {
        id: 'run_' + String(index).padStart(4, '0'),
        items,
        itemsView: 'full',
        status,
        error: null,
        startedAt: ts,
        completedAt: status === 'inProgress' ? null : ts + 8,
        durationMs: status === 'inProgress' ? null : 8000,
    };
}
export const taskRunSource = {
    thread: {
        id: TASK_RUN_THREAD_ID,
        preview: '多引擎实测 · 运行记录',
        model: 'MiniMax M3.1 Flash',
        modelProvider: 'minimax',
        cwd: '~/projects',
        createdAt: runStarts[0],
        updatedAt: runStarts[TASK_RUN_TURN_COUNT - 1],
        status: 'completed',
    },
    totalTurns: TASK_RUN_TURN_COUNT,
    startedAt: (i) => runStarts[i],
    turnAt: runTurnAt,
};
/** Kind 的工作日志（§6.3）：右栏顶部的「Kind 最近做了什么」。 */
export function getKindLog() {
    return kindLog;
}
/** 「在做的事」右栏要的三个项目的线路与站。 */
export function inProgressProjects() {
    return rootTaskIds.map((id) => tasks[id]);
}
/** 出现的成果，第 2 期用。 */
export function artifactOf(name) {
    return artifacts[name] ?? null;
}
