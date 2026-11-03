import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 任务页的横向地铁线（§4.1；brief §四.2 的通道分工）：
 * - 能同时进行的站叠在同一列；列与列之间的箭头表示「前面的完成后才能开始后面的」；
 * - 每站显示：状态点（只在站角）、名称、谁在做；有子任务再显示「2 / 3 项」和小进度条；
 * - 点一站进入下一层（同一个组件再画一遍）。
 * 画法是中性描边 + 线色只做强调，不做整站染色。
 */
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
const DOT = {
    done: 'var(--done)',
    run: 'var(--run)',
    wait: 'var(--wait)',
    fail: 'var(--fail)',
    todo: 'var(--ink-3)',
};
export function TaskLine({ detail }) {
    const ids = new Set(detail.children.map((child) => child.id));
    const depth = new Map();
    const computeDepth = (id) => {
        const cached = depth.get(id);
        if (cached !== undefined)
            return cached;
        const child = detail.children.find((item) => item.id === id);
        if (!child)
            return 0;
        depth.set(id, 0);
        const deps = child.deps.filter((dep) => ids.has(dep));
        const value = deps.length ? Math.max(...deps.map((dep) => computeDepth(dep))) + 1 : 0;
        depth.set(id, value);
        return value;
    };
    detail.children.forEach((child) => computeDepth(child.id));
    const maxDepth = Math.max(0, ...detail.children.map((child) => depth.get(child.id) ?? 0));
    const columns = Array.from({ length: maxDepth + 1 }, (_, index) => detail.children.filter((child) => (depth.get(child.id) ?? 0) === index));
    return (_jsx("div", { className: "flex min-w-0 items-start gap-2 overflow-x-auto pb-1", "data-task-line": "true", children: columns.map((column, columnIndex) => (_jsxs("div", { className: "flex items-center gap-2", children: [_jsx("div", { className: "flex min-w-[210px] flex-col gap-2", children: column.map((child) => (_jsxs("button", { type: "button", onClick: () => go('task:' + child.id), className: cn('rounded-card border bg-pane px-3 py-2 text-left transition-colors hover:bg-raised', child.state === 'wait' ? 'border-wait/45' : child.state === 'fail' ? 'border-fail/45' : 'border-line-2'), children: [_jsxs("span", { className: "flex min-w-0 items-center gap-1.5", children: [_jsx("span", { className: "size-[7px] shrink-0 rounded-full", style: { backgroundColor: DOT[child.state] } }), _jsx("span", { className: "truncate text-3 text-ink", children: child.title })] }), _jsxs("span", { className: "mt-0.5 block truncate text-1 text-ink-3", children: [child.role || '—', child.note ? ' · ' + child.note : ''] }), child.progress ? (_jsxs(_Fragment, { children: [_jsxs("span", { className: "mt-0.5 block text-1 text-ink-3", children: [child.progress.done, " / ", child.progress.total, " \u9879"] }), _jsx("span", { className: "mt-1 block h-[3px] w-full overflow-hidden rounded-full bg-hover", children: _jsx("span", { className: "block h-full rounded-full", style: {
                                                width: (child.progress.done / Math.max(child.progress.total, 1)) * 100 + '%',
                                                backgroundColor: detail.color,
                                            } }) })] })) : null] }, child.id))) }), columnIndex < columns.length - 1 ? (_jsx("span", { className: "shrink-0 select-none text-1 text-ink-3", "aria-hidden": "true", children: "\u2192" })) : null] }, columnIndex))) }));
}
