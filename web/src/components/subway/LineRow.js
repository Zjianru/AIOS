import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { cn } from '@/lib/utils';
const DOT = {
    done: 'var(--done)',
    run: 'var(--run)',
    wait: 'var(--wait)',
    fail: 'var(--fail)',
    todo: 'var(--ink-3)',
};
export function LineRow({ color, stations, onGo, className }) {
    return (_jsx("div", { className: cn('flex min-w-0 items-stretch gap-1 overflow-x-auto pb-0.5', className), "data-line-row": "true", children: stations.map((station, index) => (_jsxs("div", { className: "flex min-w-0 shrink-0 items-center gap-1", children: [_jsxs("button", { type: "button", onClick: () => station.href && onGo?.(station.href), className: "relative min-w-[118px] max-w-[190px] rounded-info border border-line-2 bg-pane px-2.5 py-1.5 text-left transition-colors hover:border-ink-3/50 hover:bg-raised", title: station.title, children: [_jsx("span", { className: "absolute right-1.5 top-1.5 size-[7px] rounded-full", style: { backgroundColor: DOT[station.state] } }), _jsx("span", { className: "block truncate pr-3 text-3 leading-4 text-ink", children: station.title }), _jsxs("span", { className: "mt-0.5 block truncate text-1 leading-3.5 text-ink-3", children: [station.role ? station.role : '', station.role && station.meta ? ' · ' : '', station.meta ?? ''] }), station.progress ? (_jsx("span", { className: "mt-1 block h-[3px] w-full overflow-hidden rounded-full bg-hover", children: _jsx("span", { className: "block h-full rounded-full", style: { width: (station.progress.done / Math.max(station.progress.total, 1)) * 100 + '%', backgroundColor: color } }) })) : null] }), index < stations.length - 1 ? (_jsx("span", { className: "shrink-0 select-none text-1 leading-none text-ink-3", "aria-hidden": "true", children: "\u2192" })) : null] }, station.id))) }));
}
