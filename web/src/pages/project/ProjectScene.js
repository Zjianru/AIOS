import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 项目页（#project:<id>；§4.5 里程碑 / §4.6 成果；brief §四）。
 *
 * 默认布局「项目页 | 对话（可收起）」（layout-motion-v1.md §一）：
 * - 左边：项目简报、里程碑（已通过的折成一行，进行中的展开看成功标准与任务）、成果；
 * - 右边：项目对话（原型是样例；「收起」把对话收成一条，第 3 期由 Dock 接管真正的收起）。
 * 数据只从 api 取（getProject / getTaskDetail 类似）。
 */
import { useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, Plus, SendHorizontal } from 'lucide-react';
import { toast } from 'sonner';
import { getProject } from '@/data/api';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { StartThingDialog } from '@/pages/work/StartThingDialog';
import { cn } from '@/lib/utils';
const STATE_LABEL = {
    todo: '排队',
    run: '在做',
    wait: '等你',
    fail: '出了问题',
    done: '完成',
};
const STATE_TONE = {
    todo: 'bg-hover text-ink-2',
    run: 'bg-run-soft text-run',
    wait: 'bg-wait-soft text-wait',
    fail: 'bg-fail-soft text-fail',
    done: 'bg-done-soft text-done',
};
const TYPE_GRADIENT = [
    'linear-gradient(135deg, #7c5cf5 0%, #c7b6ff 100%)',
    'linear-gradient(135deg, #1aa3ba 0%, #9fe3ee 100%)',
    'linear-gradient(135deg, #b9764a 0%, #f0c9a8 100%)',
    'linear-gradient(135deg, #4c8dff 0%, #b8d0ff 100%)',
];
function MilestoneSection({ milestone }) {
    const [open, setOpen] = useState(milestone.state !== 'passed');
    const passed = milestone.state === 'passed';
    return (_jsxs("section", { className: "rounded-card border border-line bg-pane/40", children: [_jsxs("button", { type: "button", onClick: () => setOpen((value) => !value), className: "flex w-full flex-wrap items-center gap-2 px-3 py-2.5 text-left transition-colors hover:bg-hover", children: [passed ? _jsx("span", { className: "text-done", children: "\u2713" }) : _jsx("span", { children: "\uD83D\uDEA9" }), _jsx("span", { className: "text-3 text-ink", children: milestone.name }), _jsx("span", { className: "rounded-full bg-hover px-2 py-[1px] text-1 text-ink-3", children: passed ? '已通过' : milestone.state === 'judging' ? '判断中' : '进行中' }), milestone.when ? _jsx("span", { className: "text-1 text-ink-3", children: milestone.when }) : null, milestone.due ? _jsx("span", { className: "text-1 text-ink-3", children: milestone.due }) : null, _jsxs("span", { className: "ml-auto text-1 text-ink-3", children: [milestone.tasks.length, " \u4EF6\u4E8B"] }), _jsx(ChevronRight, { className: cn('size-3.5 text-ink-3 transition-transform', open && 'rotate-90') })] }), open ? (_jsxs("div", { className: "border-t border-line px-3 py-2.5", children: [milestone.criteria.length ? (_jsx("ul", { className: "space-y-1", children: milestone.criteria.map(([text, state, evidence]) => (_jsxs("li", { className: "flex items-baseline gap-2 text-2", children: [_jsx("span", { className: cn('shrink-0', state === 'ok' ? 'text-done' : state === 'no' ? 'text-fail' : 'text-wait'), children: state === 'ok' ? '✓' : state === 'no' ? '✗' : '○' }), _jsx("span", { className: "text-ink-2", children: text }), evidence ? _jsx("span", { className: "min-w-0 truncate text-1 text-ink-3", children: evidence }) : null] }, text))) })) : (_jsx("p", { className: "text-2 text-ink-3", children: "\u8FD9\u4E2A\u91CC\u7A0B\u7891\u6CA1\u6709\u5217\u6210\u529F\u6807\u51C6\u3002" })), milestone.tasks.length ? (_jsx("div", { className: "mt-2.5 space-y-1.5", children: milestone.tasks.map((task) => (_jsxs("button", { type: "button", onClick: () => go('task:' + task.id), className: "flex w-full items-center gap-2 rounded-info border border-line bg-content/40 px-2.5 py-2 text-left transition-colors hover:bg-hover", children: [_jsxs("span", { className: "min-w-0 flex-1", children: [_jsx("span", { className: "block truncate text-2 text-ink", children: task.title }), _jsxs("span", { className: "mt-0.5 block truncate text-1 text-ink-3", children: [task.role, task.note ? ' · ' + task.note : ''] }), task.progress ? (_jsx("span", { className: "mt-1 block h-[3px] w-40 overflow-hidden rounded-full bg-hover", children: _jsx("span", { className: "block h-full rounded-full bg-run", style: { width: (task.progress.done / Math.max(task.progress.total, 1)) * 100 + '%' } }) })) : null] }), _jsx("span", { className: cn('shrink-0 rounded-full px-2 py-[1px] text-1', STATE_TONE[task.state]), children: STATE_LABEL[task.state] }), _jsx(ChevronRight, { className: "size-3.5 shrink-0 text-ink-3" })] }, task.id))) })) : (_jsx("p", { className: "mt-2 text-2 text-ink-3", children: "\u8FD8\u6CA1\u6709\u4EFB\u52A1\u3002" }))] })) : null] }));
}
function ProjectMainPane() {
    const projectId = useRouteStore((state) => state.route.id);
    const [detail, setDetail] = useState(undefined);
    const [startOpen, setStartOpen] = useState(false);
    useEffect(() => {
        let alive = true;
        setDetail(undefined);
        void (async () => {
            const next = await getProject(projectId);
            if (alive)
                setDetail(next);
        })();
        return () => {
            alive = false;
        };
    }, [projectId]);
    if (detail === undefined) {
        return _jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u9879\u76EE\u2026" });
    }
    if (!detail) {
        return _jsxs("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: ["\u6CA1\u6709\u8FD9\u4E2A\u9879\u76EE\uFF1A", projectId] });
    }
    return (_jsxs("div", { className: "h-full overflow-y-auto", "data-scene-page": "project", children: [_jsxs("div", { className: "mx-auto w-full max-w-[860px] px-6 py-5", children: [_jsxs("div", { className: "flex flex-wrap items-center gap-2", children: [_jsx("span", { className: "grid size-[18px] place-items-center rounded-badge brand-type text-[10px] font-semibold text-white", style: { backgroundColor: detail.color }, children: detail.code }), _jsx("h1", { className: "text-5 font-semibold leading-7 text-ink", children: detail.name }), _jsxs("span", { className: "text-1 text-ink-3", children: ["\u5B8C\u6210 ", detail.counts.done, " / ", detail.counts.total] }), _jsxs("button", { type: "button", onClick: () => setStartOpen(true), className: "ml-auto flex h-7 items-center gap-1 rounded-btn bg-ink px-2.5 text-2 text-content transition-opacity hover:opacity-90", children: [_jsx(Plus, { className: "size-3.5" }), "\u4EA4\u7ED9 Kind \u4E00\u4EF6\u4E8B"] })] }), _jsx("p", { className: "mt-1.5 max-w-[620px] text-2 leading-5 text-ink-2", children: detail.brief }), _jsx("div", { className: "mt-5 space-y-2", children: detail.milestones.map((milestone) => (_jsx(MilestoneSection, { milestone: milestone }, milestone.id))) }), _jsxs("section", { className: "mt-6", children: [_jsx("h2", { className: "text-3 font-medium text-ink", children: "\u6210\u679C" }), detail.artifacts.length ? (_jsx("div", { className: "mt-2 grid grid-cols-1 gap-2 sm:grid-cols-2", children: detail.artifacts.map((artifact, index) => (_jsxs("div", { className: "rounded-card border border-line bg-pane/40 p-2", children: [_jsx("div", { className: "grid h-[76px] place-items-center rounded-info text-1 text-white/90", style: { background: TYPE_GRADIENT[index % TYPE_GRADIENT.length] }, children: artifact.name.split('.').pop()?.toUpperCase() }), _jsx("div", { className: "mt-1.5 truncate text-2 text-ink", title: artifact.name, children: artifact.name }), _jsxs("div", { className: "mt-0.5 flex items-center justify-between text-1 text-ink-3", children: [_jsxs("span", { children: [_jsx("span", { className: "brand-type", children: artifact.version }), " \u00B7 ", artifact.size] }), _jsx("span", { className: artifact.status === 'acc' ? 'text-done' : artifact.status === 'pend' ? 'text-wait' : 'text-ink-3', children: artifact.status === 'acc' ? '✓✓ 已接受' : artifact.status === 'pend' ? '待你接受' : '草稿' })] }), _jsxs("div", { className: "mt-0.5 truncate text-1 text-ink-3", children: ["\u6765\u81EA\uFF1A", artifact.from] })] }, artifact.name + artifact.from))) })) : (_jsx("p", { className: "mt-2 text-2 text-ink-3", children: "\u8FD8\u6CA1\u6709\u6210\u679C\u3002" }))] }), _jsxs("section", { className: "mt-6", children: [_jsx("h2", { className: "text-3 font-medium text-ink", children: "\u8D44\u6599" }), _jsxs("p", { className: "mt-0.5 text-1 text-ink-3", children: ["\u653E\u5728\u8FD9\u4E2A\u9879\u76EE\u91CC\u7684\u8D44\u6599\uFF1B\u8FD8\u80FD\u67E5\u5168\u5C40\u7684 ", detail.globalMaterials, " \u4EFD\u3002"] }), detail.materials.length ? (_jsx("div", { className: "mt-2 overflow-hidden rounded-card border border-line", children: detail.materials.map((material) => (_jsxs("button", { type: "button", onClick: () => go('read:' + material.name), className: "flex w-full items-center gap-2 border-b border-line px-3 py-2 text-left transition-colors last:border-0 hover:bg-hover", children: [_jsx("span", { className: "rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3", children: material.type }), _jsx("span", { className: "min-w-0 flex-1 truncate text-2 text-ink-2", children: material.name }), _jsx("span", { className: "brand-type text-1 text-ink-3", children: material.version }), _jsx("span", { className: "rounded-full bg-hover px-2 py-[1px] text-1 text-ink-3", children: material.mode }), _jsx("span", { className: cn('size-1.5 shrink-0 rounded-full', material.status === 'ok' ? 'bg-done' : material.status === 'busy' ? 'bg-wait' : 'bg-fail') })] }, material.name))) })) : (_jsx("p", { className: "mt-2 text-2 text-ink-3", children: "\u8FD9\u4E2A\u9879\u76EE\u91CC\u8FD8\u6CA1\u6709\u8D44\u6599\u3002" }))] }), _jsx("p", { className: "mt-6 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u9879\u76EE\u9875\uFF1A\u91CC\u7A0B\u7891\u628A\u4EFB\u52A1\u5206\u6BB5\uFF1B\u91CC\u7A0B\u7891\u6CA1\u901A\u8FC7\u65F6\u540E\u7EED\u4EFB\u52A1\u505C\u4E0B\uFF0C\u8FDB\u4E86\u300C\u7B49\u4F60\u51B3\u5B9A\u300D\u3002" })] }), _jsx(StartThingDialog, { open: startOpen, onOpenChange: setStartOpen })] }));
}
function ProjectChatPane() {
    const projectId = useRouteStore((state) => state.route.id);
    const [detail, setDetail] = useState(undefined);
    const [open, setOpen] = useState(true);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const next = await getProject(projectId);
            if (alive)
                setDetail(next);
        })();
        return () => {
            alive = false;
        };
    }, [projectId]);
    if (!detail) {
        return (_jsx("div", { className: "flex h-full items-start bg-content px-4 py-3 text-2 text-ink-3", "data-pane": "chat", children: detail === undefined ? '正在取项目对话…' : '没有这个项目。' }));
    }
    if (!open) {
        return (_jsx("div", { className: "flex h-full flex-col items-center justify-start gap-2 bg-content px-3 py-3", "data-pane": "chat", children: _jsxs("button", { type: "button", onClick: () => setOpen(true), className: "flex w-full items-center gap-1.5 rounded-btn px-1.5 py-1 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2", children: [_jsx(ChevronDown, { className: "size-3.5 rotate-90" }), "\u9879\u76EE\u5BF9\u8BDD \u00B7 \u5C55\u5F00"] }) }));
    }
    const openMilestone = detail.milestones.find((milestone) => milestone.state !== 'passed');
    const nextTask = openMilestone?.tasks.find((task) => task.state !== 'done');
    const waitingTask = openMilestone?.tasks.find((task) => task.state === 'wait' || task.state === 'fail');
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col overflow-hidden bg-content", "data-pane": "chat", children: [_jsxs("header", { className: "flex h-10 shrink-0 items-center gap-2 border-b border-line px-3", children: [_jsx("span", { className: "text-2 text-ink-2", children: "\u9879\u76EE\u5BF9\u8BDD" }), _jsx("span", { className: "brand-type text-1 text-ink-3", children: detail.code }), _jsx("button", { type: "button", onClick: () => setOpen(false), className: "ml-auto rounded-btn px-1.5 py-0.5 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2", children: "\u6536\u8D77" })] }), _jsxs("div", { className: "min-h-0 flex-1 space-y-3 overflow-y-auto px-3 py-3 text-2 leading-5", children: [_jsxs("div", { className: "ml-auto max-w-[86%] rounded-card bg-hover px-2.5 py-2 text-ink", children: ["\u5E2E\u6211\u628A\u300C", detail.name, "\u300D\u63A8\u8FDB\u4E00\u4E0B\uFF0C\u5148\u628A\u300C", openMilestone?.name ?? '下一件事', "\u300D\u8FC7\u6389\u3002"] }), _jsxs("div", { className: "max-w-[92%] text-ink-2", children: [_jsx("span", { className: "text-1 text-ink-3", children: "Kind" }), _jsxs("p", { className: "mt-0.5", children: ["\u597D\u3002\u5DF2\u7ECF\u5B8C\u6210 ", detail.counts.done, " / ", detail.counts.total, " \u9879\u3002", nextTask ? ' 下一站是「' + nextTask.title + '」，我接着做。' : ' 现在没有在跑的站。'] })] }), _jsxs("div", { className: "max-w-[92%] text-ink-2", children: [_jsx("span", { className: "text-1 text-ink-3", children: "Kind" }), _jsx("p", { className: "mt-0.5", children: waitingTask
                                    ? '「' + waitingTask.title + '」要你先定一下；定完我再往下走。'
                                    : '没有要你定的事；做完我会把结果放到成果里。' })] }), _jsx("p", { className: "text-1 text-ink-3", children: "\u539F\u578B\uFF1A\u9879\u76EE\u5BF9\u8BDD\u662F\u6837\u4F8B\uFF0C\u7B2C 3 \u671F\u63A5\u771F\u5B9E\u9879\u76EE\u4F1A\u8BDD\u3002" })] }), _jsx("div", { className: "shrink-0 border-t border-line px-3 py-2", children: _jsxs("div", { className: "flex items-end gap-1.5 rounded-btn border border-line-2 bg-pane px-2 py-1.5", children: [_jsx("textarea", { rows: 1, placeholder: '和 Kind 说这个项目的事…', className: "max-h-24 min-h-5 flex-1 resize-none bg-transparent text-2 text-ink outline-none placeholder:text-ink-3" }), _jsx("button", { type: "button", onClick: () => toast('（原型）项目对话只做样例；发送等第 3 期接真实会话'), className: "grid size-6 shrink-0 place-items-center rounded-full bg-hover text-ink-3 transition-colors hover:text-ink-2", title: "\u53D1\u9001\uFF08\u539F\u578B\uFF09", children: _jsx(SendHorizontal, { className: "size-3.5" }) })] }) })] }));
}
export function ProjectScene({ pane }) {
    if (pane === 'chat')
        return _jsx(ProjectChatPane, {});
    return _jsx(ProjectMainPane, {});
}
