import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * Kind 右栏「聚焦」（brief §四.1「第 2 期追加」第 1 条）。
 *
 * 上半块：只放要你看的和正在动的——等你 → 出了问题 → 在跑，每组最多 3 条；
 * 其余正常推进折成一行；一条都没有就只显示「都在正常推进」，高度随内容收缩。
 * 数据只从 api 取；归约在 focusModel.ts，组件里不做业务计算。
 */
import { useEffect, useState } from 'react';
import { listInProgressTasks, listProjects, listTasks } from '@/data/api';
import { go } from '@/app/navigate';
import { MiniLineStrip } from '@/components/subway/MiniLineStrip';
import { buildFocusView } from './focusModel';
const STATE_COLOR = {
    wait: 'var(--wait)',
    fail: 'var(--fail)',
    run: 'var(--run)',
};
const GROUP_LABEL = {
    wait: '等你',
    fail: '出了问题',
    run: '在跑',
};
function FocusRow({ item }) {
    return (_jsxs("button", { type: "button", onClick: () => go(item.href), className: "mt-0.5 block w-full rounded-info px-2 py-1.5 text-left transition-colors hover:bg-hover", children: [_jsxs("span", { className: "flex items-baseline justify-between gap-2", children: [_jsx("span", { className: "truncate text-2 text-ink", children: item.title }), _jsx("span", { className: "shrink-0 text-1 text-ink-3", children: item.right })] }), _jsxs("span", { className: "mt-0.5 flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: "grid size-[14px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: { backgroundColor: item.project.color }, children: item.project.code }), _jsx("span", { className: "truncate", children: item.meta })] }), item.strip ? _jsx(MiniLineStrip, { strip: item.strip }) : null] }));
}
export function FocusPane() {
    const [view, setView] = useState(null);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const [roots, tasks, projects] = await Promise.all([listInProgressTasks(), listTasks(), listProjects()]);
            if (!alive)
                return;
            setView(buildFocusView(roots, tasks, projects));
        })();
        return () => {
            alive = false;
        };
    }, []);
    const summary = (() => {
        if (!view || view.allNormal)
            return '';
        const parts = [];
        if (view.toSee > 0)
            parts.push('要你看的 ' + view.toSee + ' 件');
        if (view.running > 0)
            parts.push('在跑 ' + view.running + ' 件');
        if (view.rest > 0)
            parts.push('其余 ' + view.rest + ' 站正常');
        return parts.join(' · ');
    })();
    return (_jsxs("section", { className: "shrink-0 border-b border-line", "data-block": "focus", children: [_jsxs("header", { className: "px-4 pt-3", children: [_jsx("div", { className: "flex items-baseline gap-2", children: _jsx("h3", { className: "text-3 font-medium text-ink", children: "\u805A\u7126" }) }), summary ? _jsx("p", { className: "mt-0.5 text-1 text-ink-3", children: summary }) : null] }), !view ? (_jsx("div", { className: "px-4 py-2 text-2 text-ink-3", children: "\u6B63\u5728\u6838\u5BF9\u2026" })) : view.allNormal ? (_jsx("div", { className: "px-4 pb-3 pt-2 text-2 text-ink-3", children: "\u90FD\u5728\u6B63\u5E38\u63A8\u8FDB" })) : (_jsxs("div", { className: "max-h-[38vh] overflow-y-auto px-4 pb-3 pt-2", children: [view.groups.map((group) => (_jsxs("div", { className: "mt-2 first:mt-0", children: [_jsxs("div", { className: "flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: "size-[7px] rounded-full", style: { backgroundColor: STATE_COLOR[group.state] } }), GROUP_LABEL[group.state]] }), _jsxs("div", { className: "mt-0.5", children: [group.items.map((item) => (_jsx(FocusRow, { item: item }, item.project.id + ':' + item.id))), group.more > 0 ? _jsxs("div", { className: "px-2 pt-1 text-1 text-ink-3", children: ["\u8FD8\u6709 ", group.more, " \u4EF6"] }) : null] })] }, group.state))), view.rest > 0 ? (_jsxs("button", { type: "button", onClick: () => go('work'), className: "mt-2 flex w-full items-center justify-between border-t border-line pt-2 text-1 text-ink-3 transition-colors hover:text-ink-2", children: [_jsxs("span", { children: ["\u5176\u4F59 ", view.rest, " \u7AD9\u6B63\u5E38\u63A8\u8FDB"] }), _jsx("span", { children: "\u5168\u90E8 \u203A" })] })) : null] }))] }));
}
