import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 工作页（§5 看板；brief §四「工作」）。
 *
 * 默认布局「看板/日历 | 等你」（layout-motion-v1.md §一）。
 * - 看板四列：排队 / 在做 / 等你 / 完成（近 7 天）；等你列的内容和首页「等你决定」相同；
 * - 可按 全部项目 / 个人 / 某个项目 筛；
 * - 日历与唤醒的显示部分归第 3 期（§四 p3），这里只放一句说明，不放假日历；
 * - 「交给 Kind 一件事」打开三档露脸对话框（§四.4）。
 * 任务的地铁线、这句话的账、里程碑、暂停/改计划在任务页（#task:<id>）。
 */
import { useEffect, useState } from 'react';
import { ChevronDown, Plus } from 'lucide-react';
import { toast } from 'sonner';
import { getBoard, getNow, listProjects } from '@/data/api';
import { useRouteStore } from '@/app/routeStore';
import { go } from '@/app/navigate';
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger, } from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';
import { StartThingDialog } from './StartThingDialog';
const WORD_TONE = {
    '待你接受': 'text-wait',
    '出问题 · Kind 处理中': 'text-fail',
    '在做': 'text-run',
    '检查中': 'text-run',
};
function CodeChip({ code, color }) {
    return (_jsx("span", { className: "grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: { backgroundColor: color }, children: code }));
}
function BoardTile({ card }) {
    return (_jsxs("button", { type: "button", onClick: () => go('task:' + card.id), className: "w-full rounded-card border border-line bg-pane/50 p-2.5 text-left transition-colors hover:border-ink-3/40 hover:bg-raised", children: [_jsxs("span", { className: "flex min-w-0 items-center gap-1.5 text-1 text-ink-3", children: [_jsx(CodeChip, { code: card.projectCode, color: card.color }), _jsx("span", { className: "truncate", children: card.projectName })] }), _jsx("span", { className: "mt-1 block truncate text-2 text-ink", title: card.title, children: card.title }), card.path ? (_jsx("span", { className: "mt-0.5 block truncate text-1 text-ink-3", title: card.path, children: card.path })) : null, _jsxs("span", { className: "mt-1 flex items-center justify-between gap-2 text-1 text-ink-3", children: [_jsx("span", { className: "truncate", children: card.role || '—' }), _jsx("span", { className: cn('shrink-0', WORD_TONE[card.word]), children: card.word })] }), card.artifact ? (_jsxs("span", { className: "mt-0.5 block truncate text-1 text-ink-3", children: [card.artifact.name, " ", _jsx("span", { className: "brand-type", children: card.artifact.version }), " \u00B7 ", card.artifact.size] })) : null] }));
}
/** 看板「等你」列的紧凑卡：内容还是首页「等你决定」那几件，只是列窄，点开去任务页看完整选项。 */
function WaitTile({ decision, color }) {
    return (_jsxs("button", { type: "button", onClick: () => go(decision.href), className: "w-full rounded-card border border-wait/30 bg-wait-soft/30 p-2.5 text-left transition-colors hover:bg-wait-soft/60", children: [_jsxs("span", { className: "flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: "text-wait", children: decision.typeLabel }), decision.fromProblem ? _jsx("span", { className: "rounded-badge bg-wait-soft px-1 py-[1px] text-wait", children: "\u56E0\u95EE\u9898\u800C\u6765" }) : null, _jsx("span", { className: "ml-auto brand-type", children: decision.time })] }), _jsx("span", { className: "mt-1 block text-2 leading-5 text-ink", children: decision.title }), _jsxs("span", { className: "mt-0.5 flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx(CodeChip, { code: decision.projectCode ?? '—', color: color }), _jsx("span", { className: "truncate", children: decision.projectName }), _jsx("span", { className: "ml-auto shrink-0", children: "\u53BB\u770B \u203A" })] })] }));
}
function CalendarPane() {
    return (_jsx("div", { className: "grid min-h-0 flex-1 place-items-center px-6", children: _jsxs("div", { className: "max-w-[360px] text-center", children: [_jsx("div", { className: "text-3 text-ink-2", children: "\u65E5\u5386\u4E0E\u5524\u9192 \u00B7 \u7B2C 3 \u671F" }), _jsx("p", { className: "mt-1 text-1 leading-4 text-ink-3", children: "\u8FD9\u91CC\u662F\u4EFB\u52A1\u7684\u65F6\u95F4\u5B89\u6392\u4E0E\u5524\u9192\uFF08\u663E\u793A\u90E8\u5206\u7167 v13-o O3\uFF09\u3002\u7B2C 2 \u671F\u5148\u4E0D\u505A\uFF1B\u5207\u56DE\u300C\u770B\u677F\u300D\u80FD\u770B\u5230\u5168\u90E8\u4EFB\u52A1\u3002" })] }) }));
}
function BoardPane() {
    const [columns, setColumns] = useState(null);
    const [now, setNow] = useState(null);
    const [projects, setProjects] = useState({});
    // #work?wv=cal 要能直接打开日历；点击看板/日历是本地状态。
    const route = useRouteStore((state) => state.route);
    const wantedView = route.query.get('wv') === 'cal' ? 'cal' : 'board';
    const [view, setView] = useState(wantedView);
    useEffect(() => {
        setView(wantedView);
    }, [wantedView]);
    const [filter, setFilter] = useState('all');
    const [startOpen, setStartOpen] = useState(false);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const [boardView, nowView, projectList] = await Promise.all([getBoard(), getNow(), listProjects()]);
            if (!alive)
                return;
            setColumns(boardView);
            setNow(nowView);
            setProjects(projectList);
        })();
        return () => {
            alive = false;
        };
    }, []);
    if (!columns || !now) {
        return _jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u5DE5\u4F5C\u9875\u2026" });
    }
    const projectNameToId = new Map(Object.values(projects).map((project) => [project.name, project.id]));
    const keepProject = (projectId) => {
        if (filter === 'all')
            return true;
        if (filter === 'me')
            return false;
        return projectId === filter;
    };
    const keepDecision = (decision) => {
        if (filter === 'all')
            return true;
        if (filter === 'me')
            return true;
        return projectNameToId.get(decision.projectName ?? '') === filter;
    };
    const queue = columns.queue.filter((card) => keepProject(card.projectId));
    const doing = columns.doing.filter((card) => keepProject(card.projectId));
    const done = columns.done.filter((card) => keepProject(card.projectId));
    const decisions = now.decisions.filter(keepDecision);
    const filterLabel = filter === 'all'
        ? '全部项目'
        : filter === 'me'
            ? '个人'
            : Object.values(projects).find((project) => project.id === filter)?.name ?? '全部项目';
    const groups = [
        { key: 'queue', label: '排队', tone: 'var(--ink-3)', hint: '等前置 / 缺条件', count: queue.length },
        { key: 'doing', label: '在做', tone: 'var(--run)', hint: '执行中、待检查', count: doing.length },
        { key: 'wait', label: '等你', tone: 'var(--wait)', hint: '只放需要你的', count: decisions.length },
        { key: 'done', label: '完成', tone: 'var(--done)', hint: '近 7 天', count: done.length },
    ];
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col bg-content", "data-scene-page": "work", children: [_jsxs("header", { className: "flex h-11 shrink-0 items-center gap-1.5 border-b border-line px-3 text-3", children: [_jsx("span", { className: "text-ink", children: "\u5DE5\u4F5C" }), _jsxs(DropdownMenu, { children: [_jsx(DropdownMenuTrigger, { asChild: true, children: _jsxs("button", { type: "button", className: "flex h-7 items-center gap-1 rounded-btn px-2 text-2 text-ink-2 transition-colors hover:bg-hover", children: [filterLabel, " ", _jsx(ChevronDown, { className: "size-3.5" })] }) }), _jsx(DropdownMenuContent, { align: "start", className: "w-56", children: _jsxs(DropdownMenuRadioGroup, { value: filter, onValueChange: setFilter, children: [_jsx(DropdownMenuRadioItem, { value: "all", children: "\u5168\u90E8\u9879\u76EE" }), _jsx(DropdownMenuRadioItem, { value: "me", children: "\u4E2A\u4EBA" }), Object.values(projects).map((project) => (_jsx(DropdownMenuRadioItem, { value: project.id, children: project.name }, project.id)))] }) })] }), _jsx("span", { className: "ml-auto" }), _jsx("div", { className: "flex items-center rounded-btn border border-line p-[2px]", children: ['board', 'cal'].map((key) => (_jsx("button", { type: "button", onClick: () => setView(key), className: cn('rounded-badge px-2 py-0.5 text-2 transition-colors', view === key ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink-2'), children: key === 'board' ? '看板' : '日历' }, key))) }), _jsxs("button", { type: "button", onClick: () => setStartOpen(true), className: "ml-1 flex h-7 items-center gap-1 rounded-btn bg-ink px-2.5 text-2 text-content transition-opacity hover:opacity-90", children: [_jsx(Plus, { className: "size-3.5" }), "\u4EA4\u7ED9 Kind \u4E00\u4EF6\u4E8B"] })] }), view === 'cal' ? (_jsx(CalendarPane, {})) : (_jsx("div", { className: "grid min-h-0 flex-1 grid-cols-4 gap-2.5 overflow-hidden p-3", children: groups.map((group) => (_jsxs("section", { className: "flex min-h-0 min-w-0 flex-col rounded-card border border-line/70 bg-pane/30 p-2", children: [_jsxs("header", { className: "mb-1.5 flex shrink-0 flex-wrap items-baseline gap-x-1.5 gap-y-0 text-1", children: [_jsx("span", { className: "size-[7px] rounded-full", style: { backgroundColor: group.tone } }), _jsx("span", { className: "text-2 text-ink", children: group.label }), _jsx("span", { className: "brand-type text-ink-3", children: group.count }), _jsx("span", { className: "ml-auto text-ink-3", children: group.hint })] }), _jsxs("div", { className: "min-h-0 flex-1 space-y-2 overflow-y-auto pr-0.5", children: [group.key === 'queue' ? queue.map((card) => _jsx(BoardTile, { card: card }, card.id)) : null, group.key === 'doing' ? doing.map((card) => _jsx(BoardTile, { card: card }, card.id)) : null, group.key === 'done' ? done.map((card) => _jsx(BoardTile, { card: card }, card.id)) : null, group.key === 'wait'
                                    ? decisions.map((decision) => (_jsx(WaitTile, { decision: decision, color: Object.values(projects).find((project) => project.name === decision.projectName)?.color ??
                                            'var(--ink-3)' }, decision.id)))
                                    : null, group.key === 'wait' && !decisions.length ? (_jsx("p", { className: "px-1 py-2 text-1 text-ink-3", children: "\u6CA1\u6709\u8981\u4F60\u7684\u4E8B\u3002" })) : null] })] }, group.key))) })), _jsx(StartThingDialog, { open: startOpen, onOpenChange: setStartOpen })] }));
}
function WaitingPane() {
    const [now, setNow] = useState(null);
    const [allowed, setAllowed] = useState(false);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const next = await getNow();
            if (alive)
                setNow(next);
        })();
        return () => {
            alive = false;
        };
    }, []);
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col overflow-hidden bg-content", "data-pane": "waiting", children: [_jsx("header", { className: "shrink-0 border-b border-line px-4 pb-2 pt-3", children: _jsxs("div", { className: "flex items-baseline gap-2", children: [_jsx("h3", { className: "text-3 font-medium text-ink", children: "\u7B49\u4F60" }), _jsx("span", { className: "brand-type text-1 text-ink-3", children: now ? now.decisions.length : 0 }), _jsx("span", { className: "ml-auto text-1 text-ink-3", children: "\u53EA\u653E\u9700\u8981\u4F60\u7684" })] }) }), _jsx("div", { className: "min-h-0 flex-1 overflow-y-auto px-3 py-3", children: !now ? (_jsx("div", { className: "text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u2026" })) : (_jsxs(_Fragment, { children: [now.decisions.map((decision) => (_jsxs("button", { type: "button", onClick: () => go(decision.href), className: "mb-1.5 block w-full rounded-info border border-line bg-pane/40 px-2.5 py-2 text-left transition-colors hover:bg-hover", children: [_jsxs("span", { className: "flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: "text-wait", children: decision.typeLabel }), _jsx("span", { className: "truncate", children: decision.projectName }), _jsx("span", { className: "ml-auto brand-type", children: decision.time })] }), _jsx("span", { className: "mt-0.5 block truncate text-2 text-ink", children: decision.title })] }, decision.id))), _jsxs("div", { className: "mt-2 rounded-card border border-wait/30 bg-wait-soft/40 p-2.5", children: [_jsx("div", { className: "text-1 text-ink-3", children: "\u8981\u4F60\u5141\u8BB8 \u00B7 Kind" }), _jsx("div", { className: "mt-0.5 text-2 text-ink", children: "\u6E05\u7406\u6784\u5EFA\u7F13\u5B58\uFF0C\u4F1A\u5220\u9664 2.1 GB \u6587\u4EF6" }), _jsx("div", { className: "mt-1.5 flex flex-wrap gap-1.5", children: ['允许一次', '始终允许', '不允许'].map((label) => (_jsx("button", { type: "button", disabled: allowed && label === '允许一次', onClick: () => {
                                            setAllowed(true);
                                            toast('（原型）你选了：' + label);
                                        }, className: cn('rounded-btn border px-2 py-0.5 text-1 transition-colors', label === '允许一次'
                                            ? 'border-ink-3 bg-ink text-content'
                                            : 'border-line-2 text-ink-2 hover:bg-hover'), style: label === '允许一次' ? { color: 'var(--content)' } : undefined, children: label }, label))) })] })] })) }), _jsx("footer", { className: "shrink-0 border-t border-line px-4 py-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u5DE5\u4F5C\uFF1A\u770B\u677F\u56DB\u5217\u662F\u4E8B\u5B9E\u6D3E\u751F\u7684\uFF1B\u300C\u7B49\u4F60\u300D\u53EA\u653E\u9700\u8981\u4F60\u7684\uFF0C\u95EE\u9898\u9ED8\u8BA4 Kind \u5148\u5904\u7406\u3002" })] }));
}
export function WorkScene({ pane }) {
    if (pane === 'waiting')
        return _jsx(WaitingPane, {});
    return _jsx(BoardPane, {});
}
