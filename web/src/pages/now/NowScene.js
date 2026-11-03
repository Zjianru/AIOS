import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 首页「现在」（experience-spec-v1.md §3；brief §四）。
 *
 * 从上到下：等你决定（暖橙）→ 出了问题（红；分 LLM / AIOS，只陈述「发生了什么 → Kind 怎么处理」，
 * 不放决定按钮）→ 在推进（蓝；一个根任务一张卡，卡里一条地铁线）→ 本周完成一行。
 * 卡片和地铁线用的都是同一套 token；「还需」带 ≈ 的写明是估算。
 * 数据只从 api 取（getNow），不 import 样例。
 */
import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { getNow } from '@/data/api';
import { nowCounts } from '@/data/fixtures/now';
import { go } from '@/app/navigate';
import { LineRow } from '@/components/subway/LineRow';
import { cn } from '@/lib/utils';
const TONE = {
    askDecision: 'text-wait',
    artifact: 'text-done',
    askWeb: 'text-wait',
    assignRole: 'text-ink-3',
    milestone: 'text-wait',
};
const HANDLED = {
    已处理: 'text-done',
    处理中: 'text-fail',
    已转为决定: 'text-wait',
};
const HANDLED_DOT = {
    已处理: 'var(--done)',
    处理中: 'var(--fail)',
    已转为决定: 'var(--wait)',
};
const SIDE = { llm: 'LLM 问题', aios: 'AIOS 问题' };
function CodeChip({ code, color }) {
    if (!code)
        return null;
    return (_jsx("span", { className: "grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: { backgroundColor: color ?? 'var(--ink-3)' }, children: code }));
}
function Origin({ code, project, task, color }) {
    if (!project)
        return null;
    return (_jsxs("span", { className: "flex min-w-0 items-center gap-1 text-1 text-ink-3", children: [_jsx(CodeChip, { code: code, color: color }), _jsx("span", { className: "truncate", children: project }), task ? _jsxs("span", { className: "truncate opacity-80", children: [" \u203A ", task] }) : null] }));
}
/** 等你决定的卡；工作页的「等你」列也复用它（内容与首页「等你决定」相同）。 */
export function DecisionCard({ item }) {
    const act = (label) => toast('（原型）' + label + '：' + item.title);
    return (_jsxs("article", { className: "rounded-card border border-line bg-pane/60 p-3", children: [_jsxs("header", { className: "flex flex-wrap items-center gap-2 text-1 text-ink-3", children: [_jsx("span", { className: cn('font-medium', TONE[item.kind]), children: item.typeLabel }), item.fromProblem ? (_jsx("span", { className: "rounded-badge bg-wait-soft px-1.5 py-[1px] text-wait", children: "\u56E0\u95EE\u9898\u800C\u6765" })) : null, _jsx(Origin, { code: item.projectCode, project: item.projectName, task: item.taskTitle }), _jsx("span", { className: "ml-auto brand-type", children: item.time })] }), _jsx("h3", { className: "mt-1.5 text-4 font-medium leading-6 text-ink", children: item.title }), item.artifact ? (_jsxs("div", { className: "mt-1 flex items-baseline gap-2 text-2", children: [_jsx("span", { className: "text-ink-2", children: item.artifact.name }), _jsx("span", { className: "brand-type text-ink-3", children: item.artifact.version })] })) : null, item.why ? _jsx("p", { className: "mt-1 text-2 leading-5 text-ink-2", children: item.why }) : null, item.options?.length ? (_jsxs("div", { className: "mt-2 space-y-1", children: [_jsx("div", { className: "text-1 text-ink-3", children: "Kind \u7ED9\u7684\u9009\u9879 \u00B7 \u70B9\u4E00\u884C\u5C31\u662F\u9009\u5B83" }), item.options.map((option) => (_jsxs("button", { type: "button", onClick: () => act(option), className: "flex w-full items-center gap-2 rounded-lg border border-line px-2.5 py-1.5 text-left transition-colors hover:bg-hover", children: [_jsx("span", { className: "size-3 shrink-0 rounded-full border border-ink-3" }), _jsx("span", { className: "text-3 text-ink", children: option })] }, option))), _jsx("button", { type: "button", onClick: () => act('我自己说'), className: "w-full rounded-lg border border-dashed border-line-2 px-2.5 py-1.5 text-left text-3 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2", children: "\u6211\u81EA\u5DF1\u8BF4\u2026" })] })) : null, _jsxs("div", { className: "mt-2 flex items-center gap-2", children: [item.actions.map((action) => (_jsx("button", { type: "button", onClick: () => act(action.label), className: cn('rounded-btn px-2.5 py-1 text-2 transition-colors', action.primary ? 'bg-ink text-content hover:opacity-90' : 'border border-line-2 text-ink-2 hover:bg-hover'), children: action.label }, action.label))), _jsxs("button", { type: "button", onClick: () => go(item.href), className: "ml-auto flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2", children: ["\u5C55\u5F00 \u00B7 \u53BB\u770B ", _jsx("span", { "aria-hidden": "true", children: "\u203A" })] })] })] }));
}
function ProblemRow({ item }) {
    return (_jsxs("article", { className: "rounded-card border border-line bg-pane/40 p-3", children: [_jsxs("header", { className: "flex flex-wrap items-center gap-2 text-1 text-ink-3", children: [_jsx("span", { className: "text-fail", children: "\u51FA\u4E86\u95EE\u9898" }), _jsx(Origin, { code: null, project: item.projectName, task: item.taskTitle }), _jsx("span", { className: "rounded-badge bg-hover px-1.5 py-[1px]", children: SIDE[item.side] }), _jsxs("span", { className: cn('flex items-center gap-1 rounded-badge bg-hover px-1.5 py-[1px]', HANDLED[item.handled]), children: [_jsx("span", { className: "size-[6px] rounded-full", style: { backgroundColor: HANDLED_DOT[item.handled] } }), item.handled] }), _jsx("span", { className: "ml-auto brand-type", children: item.time })] }), _jsx("div", { className: "mt-1 text-3 text-ink", children: item.what }), _jsxs("div", { className: "mt-0.5 text-2 text-ink-3", children: ["Kind \u7684\u5904\u7406\uFF1A", item.how] }), _jsx("div", { className: "mt-1 flex justify-end", children: _jsxs("button", { type: "button", onClick: () => (item.href ? go(item.href) : toast('（原型）这是 AIOS 自己的问题，Kind 正在处理')), className: "flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2", children: ["\u5C55\u5F00 \u00B7 \u53BB\u770B ", _jsx("span", { "aria-hidden": "true", children: "\u203A" })] }) })] }));
}
const FLAG_TONE = {
    wait: 'bg-wait-soft text-wait',
    fail: 'bg-fail-soft text-fail',
    run: 'bg-run-soft text-run',
};
function RootCard({ root, usage }) {
    return (_jsxs("article", { className: "rounded-card border border-line bg-pane/60 p-3", children: [_jsxs("header", { className: "flex flex-wrap items-center gap-2", children: [_jsx("h3", { className: "text-4 font-medium text-ink", children: root.title }), root.subtitle ? _jsx("span", { className: "text-1 text-ink-3", children: root.subtitle }) : null, _jsx(CodeChip, { code: root.projectCode, color: root.color }), _jsx("span", { className: "text-2 text-ink-2", children: root.projectName }), root.flag ? (_jsx("span", { className: cn('ml-auto rounded-full px-2 py-[1px] text-1', FLAG_TONE[root.flag.tone]), children: root.flag.text })) : null] }), _jsx("div", { className: "mt-2.5", children: _jsx(LineRow, { color: root.color, stations: root.line, onGo: (href) => go(href) }) }), root.attention ? (_jsxs("button", { type: "button", onClick: () => go('now'), className: "mt-2 flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2", children: [_jsx("span", { "aria-hidden": "true", children: "\u21B3" }), " ", root.attention, " ", _jsx("span", { "aria-hidden": "true", children: "\u203A" })] })) : null, _jsxs("div", { className: "mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-1 text-ink-3", children: [_jsx("span", { className: "text-ink-2", children: usage.model }), _jsx("span", { className: "opacity-60", children: "\u00B7" }), _jsxs("span", { children: ["\u5DF2\u7528 ", _jsx("span", { className: "brand-type", children: usage.elapsed })] }), _jsx("span", { className: "opacity-60", children: "\u00B7" }), _jsxs("span", { children: ["\u8FD8\u9700 ", _jsx("span", { className: "brand-type", children: usage.eta })] }), _jsx("span", { className: "opacity-60", children: "\u00B7" }), _jsxs("span", { className: "brand-type", children: [usage.tokens, " token"] }), _jsx("span", { className: "opacity-60", children: "\u00B7" }), _jsx("span", { className: "brand-type", children: usage.cost }), _jsxs("button", { type: "button", onClick: () => go('project:' + root.projectId), className: "ml-auto flex items-center gap-1 text-1 text-ink-3 transition-colors hover:text-ink-2", children: ["\u5C55\u5F00 \u00B7 \u53BB\u770B ", _jsx("span", { "aria-hidden": "true", children: "\u203A" })] })] })] }));
}
function GroupHeader({ tone, label, count, hint }) {
    return (_jsxs("div", { className: "mb-2 flex flex-wrap items-center gap-2", children: [_jsx("span", { className: "size-[9px] rounded-full", style: { backgroundColor: tone } }), _jsx("h2", { className: "text-3 font-medium text-ink", children: label }), _jsx("span", { className: "brand-type text-1 text-ink-3", children: count }), hint ? _jsx("span", { className: "ml-auto text-1 text-ink-3", children: hint }) : null] }));
}
function greetingOf(hour) {
    if (hour < 6)
        return '夜深了';
    if (hour < 11)
        return '早上好';
    if (hour < 13)
        return '中午好';
    if (hour < 18)
        return '下午好';
    return '晚上好';
}
export function NowScene() {
    const [data, setData] = useState(null);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const next = await getNow();
            if (alive)
                setData(next);
        })();
        return () => {
            alive = false;
        };
    }, []);
    if (!data) {
        return _jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u300C\u73B0\u5728\u300D\u2026" });
    }
    const counts = nowCounts(data);
    const greeting = greetingOf(new Date().getHours());
    return (_jsx("div", { className: "h-full overflow-y-auto", "data-scene-page": "now", children: _jsxs("div", { className: "mx-auto w-full max-w-[860px] px-6 py-6", children: [_jsx("h1", { className: "text-6 font-semibold leading-8 text-ink", children: greeting }), _jsxs("p", { className: "mt-1 flex flex-wrap items-center gap-x-1 text-3 text-ink-2", children: [_jsx("span", { className: "brand-type", children: counts.decisions }), " \u4EF6\u7B49\u4F60\u51B3\u5B9A", _jsx("span", { className: "text-ink-3", children: "\u00B7" }), _jsx("span", { className: "brand-type", children: counts.problems }), " \u4E2A\u95EE\u9898\uFF08Kind \u5DF2\u5904\u7406 ", _jsx("span", { className: "brand-type", children: counts.fixed }), " \u4E2A\uFF09", _jsx("span", { className: "text-ink-3", children: "\u00B7" }), _jsx("span", { className: "brand-type", children: counts.running }), " \u4EF6\u5728\u63A8\u8FDB", _jsx("button", { type: "button", onClick: () => go('notify'), className: "ml-2 text-1 text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2", children: "\u4EC0\u4E48\u65F6\u5019\u4F1A\u901A\u77E5\u4F60" })] }), data.decisions.length ? (_jsxs("section", { className: "mt-6", children: [_jsx(GroupHeader, { tone: "var(--wait)", label: "\u7B49\u4F60\u51B3\u5B9A", count: data.decisions.length }), _jsx("div", { className: "space-y-2", children: data.decisions.map((item) => (_jsx(DecisionCard, { item: item }, item.id))) })] })) : null, data.problems.length ? (_jsxs("section", { className: "mt-6", children: [_jsx(GroupHeader, { tone: "var(--fail)", label: "\u51FA\u4E86\u95EE\u9898", count: data.problems.length, hint: "Kind \u4F1A\u5148\u81EA\u5DF1\u5904\u7406\uFF1B\u5904\u7406\u4E0D\u4E86\u7684\u624D\u4F1A\u53D8\u6210\u4E0A\u9762\u7684\u51B3\u5B9A" }), _jsx("div", { className: "space-y-2", children: data.problems.map((item) => (_jsx(ProblemRow, { item: item }, item.id))) })] })) : null, data.roots.length ? (_jsxs("section", { className: "mt-6", children: [_jsx(GroupHeader, { tone: "var(--run)", label: "\u5728\u63A8\u8FDB", count: data.roots.length }), _jsx("div", { className: "space-y-2", children: data.roots.map((root) => (_jsx(RootCard, { root: root, usage: data.usage[root.projectId] }, root.id))) })] })) : null, _jsxs("section", { className: "mt-6 border-t border-line pt-3", children: [_jsxs("button", { type: "button", onClick: () => go('work'), className: "flex w-full items-center gap-2 text-left text-2 text-ink-2 transition-colors hover:text-ink", children: [_jsx("span", { className: "grid size-4 shrink-0 place-items-center rounded-full bg-done-soft text-1 text-done", children: "\u2713" }), "\u672C\u5468\u5B8C\u6210 ", _jsx("span", { className: "brand-type", children: data.weekDone.count }), " \u4EF6\uFF1A", data.weekDone.names.join('、'), _jsx("span", { className: "ml-auto shrink-0 text-1 text-ink-3", children: "\u67E5\u770B \u203A" })] }), _jsx("p", { className: "mt-3 text-1 leading-4 text-ink-3", children: "\u300C\u8FD8\u9700\u300D\u5E26 \u2248 \u7684\u662F\u4F30\u7B97\uFF1A\u6309\u8FD9\u4EF6\u4E8B\u5DF2\u5B8C\u6210\u7684\u51E0\u7AD9\u5E73\u5747\u7528\u65F6\u63A8\u7B97\uFF0C\u4E0D\u542B\u7B49\u4F60\u7684\u65F6\u95F4\u3002" }), _jsx("p", { className: "mt-2 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u73B0\u5728\uFF1A\u4F60\u4E0D\u5728\u65F6\u53D1\u751F\u7684\u4E8B\u6C47\u603B\u5728\u8FD9\u91CC\uFF1B\u95EE\u9898\u9ED8\u8BA4\u7531 Kind \u5148\u5904\u7406\uFF0C\u53EA\u6709\u8981\u4F60\u62CD\u677F\u7684\u624D\u5192\u6CE1\u6210\u300C\u7B49\u4F60\u51B3\u5B9A\u300D\u3002" })] })] }) }));
}
