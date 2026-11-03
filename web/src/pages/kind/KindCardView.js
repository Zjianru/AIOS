import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 卡片：v10-tables §2 的固定五格模板 + 封闭类型表（14 种）。
 * 模板：[图标] 类型 · 来自哪 ⌛时间 / 标题 / 主体 / [主操作][次操作]   去看 ›
 * 规则：一张卡最多 1 个主操作（实心）；办完收成一行「✓ 类型 · 结果 · 时间」；
 *       会改变东西的动作给「撤销」提示条；不在表里的事不出卡片。
 */
import { memo, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
const TYPE_LABEL = {
    routeDraft: '路线草稿',
    task: '任务',
    askDecision: '等你决定',
    askApproval: '要你允许',
    askWeb: '要你去网页完成',
    artifact: '成果',
    memory: '记忆',
    wake: '唤醒',
    roleJoin: '角色加入',
    planChanged: '计划变了',
    milestone: '里程碑判断',
    assignRole: '指派角色',
    quotaAlert: '提醒线到了',
    problem: '出了问题',
};
/** 要你动手的卡有框有底色；告知类的平铺（visual-plan-v12.md §版式）。 */
const FRAMED = new Set([
    'routeDraft',
    'askDecision',
    'askApproval',
    'askWeb',
    'artifact',
    'milestone',
    'assignRole',
]);
const TONE = {
    routeDraft: 'text-line-w',
    task: 'text-run',
    askDecision: 'text-wait',
    askApproval: 'text-wait',
    askWeb: 'text-wait',
    artifact: 'text-done',
    memory: 'text-ink-3',
    wake: 'text-line-n',
    roleJoin: 'text-ink-3',
    planChanged: 'text-ink-3',
    milestone: 'text-wait',
    assignRole: 'text-ink-3',
    quotaAlert: 'text-wait',
    problem: 'text-fail',
};
function handleAction(card, label, mutates) {
    if (mutates) {
        toast('已' + label, {
            description: card.title,
            action: { label: '撤销', onClick: () => toast('已撤销') },
        });
        return;
    }
    toast(label, { description: card.title });
}
/**
 * 成果卡的主体：默认收成一行「改了什么 N 处 · 要求 x/y 满足」，点开看明细
 * （brief §四.3 信息分层：卡片里那两段默认收成一行）。
 */
function ArtifactBody({ body }) {
    const [open, setOpen] = useState(false);
    const met = body.satisfied.filter((item) => item.ok).length;
    return (_jsxs("div", { className: "mt-2", children: [_jsxs("div", { className: "flex items-baseline gap-2 text-3", children: [_jsx("span", { className: "text-ink", children: body.artifact }), _jsx("span", { className: "brand-type text-ink-3", children: body.version })] }), _jsxs("button", { type: "button", onClick: () => setOpen((value) => !value), className: "mt-1 flex items-center gap-1.5 text-1 text-ink-3 transition-colors hover:text-ink-2", children: [_jsx(ChevronRight, { className: cn('size-3 transition-transform', open && 'rotate-90') }), "\u6539\u4E86\u4EC0\u4E48 ", body.changes.length, " \u5904 \u00B7 \u8981\u6C42", ' ', _jsxs("span", { className: "brand-type", children: [met, "/", body.satisfied.length] }), ' ', "\u6EE1\u8DB3"] }), open ? (_jsxs("div", { className: "mt-1 space-y-1", children: [_jsx("ul", { className: "space-y-0.5 text-2 text-ink-2", children: body.changes.map((change) => (_jsxs("li", { children: ["\u00B7 ", change] }, change))) }), _jsx("ul", { className: "space-y-0.5 text-2", children: body.satisfied.map((item) => (_jsxs("li", { className: "flex gap-1.5", children: [_jsx("span", { className: item.ok ? 'text-done' : 'text-wait', children: item.ok ? '✓' : '◐' }), _jsx("span", { className: "text-ink-2", children: item.text })] }, item.text))) })] })) : null] }));
}
function Origin({ card }) {
    const { origin } = card;
    if (!origin.projectName)
        return _jsx("span", { children: "Kind \u7684\u5BF9\u8BDD" });
    return (_jsxs("span", { className: "flex min-w-0 items-center gap-1", children: [_jsx("span", { className: "grid size-[14px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: { background: 'var(--line-w)' }, children: origin.projectCode }), _jsx("span", { className: "truncate", children: origin.taskTitle ? origin.projectName + ' › ' + origin.taskTitle : origin.projectName })] }));
}
function Body({ body }) {
    switch (body.kind) {
        case 'routeDraft':
            return (_jsxs("div", { className: "mt-2", children: [_jsx("div", { className: "flex flex-wrap gap-1.5", children: body.stations.map((station) => (_jsxs("span", { className: "rounded-lg bg-pane px-2 py-1 text-2 text-ink-2", children: [_jsx("b", { className: "font-medium text-ink", children: station.title }), _jsx("span", { className: "mx-1 text-ink-3", children: "\u00B7" }), station.role, _jsx("span", { className: "mx-1 text-ink-3", children: "\u00B7" }), station.what] }, station.title))) }), _jsxs("dl", { className: "mt-2 grid grid-cols-[52px_1fr] gap-x-2 gap-y-1 text-2", children: [_jsx("dt", { className: "text-ink-3", children: "\u5B8C\u6210\u6807\u51C6" }), _jsx("dd", { className: "text-ink-2", children: body.criterion }), _jsx("dt", { className: "text-ink-3", children: "\u5F00\u5DE5\u524D\u4F30\u7B97" }), _jsx("dd", { className: "est text-ink-3", title: "\u6CA1\u62FF\u5230\u5B9E\u9645\u6570\u636E\uFF0C\u6309\u540C\u7C7B\u7AD9\u70B9\u7684\u5E73\u5747\u7528\u65F6\u4F30\u7B97", children: body.estimate })] })] }));
        case 'task':
            return (_jsxs("div", { className: "mt-1.5 text-2 text-ink-2", children: [_jsx("span", { className: "brand-type text-ink", children: body.progress }), _jsx("span", { className: "mx-1.5 text-ink-3", children: "\u00B7" }), "\u6B63\u5728\u505A\uFF1A", body.running] }));
        case 'askDecision':
            return (_jsxs("div", { className: "mt-2", children: [_jsx("div", { className: "mb-1 text-1 text-ink-3", children: "Kind \u7ED9\u7684\u9009\u9879 \u00B7 \u70B9\u4E00\u884C\u5C31\u662F\u9009\u5B83" }), _jsxs("div", { className: "flex flex-col gap-1", children: [body.options.map((option) => (_jsxs("button", { type: "button", onClick: () => toast('你选了：' + option.label), className: "flex items-start gap-2 rounded-lg border border-line px-2.5 py-1.5 text-left transition-colors hover:bg-hover", children: [_jsx("span", { className: "mt-[3px] size-3 shrink-0 rounded-full border border-ink-3" }), _jsxs("span", { className: "min-w-0", children: [_jsx("span", { className: "block text-3 text-ink", children: option.label }), _jsx("span", { className: "block text-2 text-ink-3", children: option.description })] })] }, option.label))), _jsx("button", { type: "button", className: "rounded-lg border border-dashed border-line-2 px-2.5 py-1.5 text-left text-3 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2", children: "\u6211\u81EA\u5DF1\u8BF4\u2026" })] })] }));
        case 'askApproval':
            return (_jsxs("div", { className: "mt-2", children: [_jsx("div", { className: "rounded-lg bg-pane px-2.5 py-1.5", children: _jsx("code", { className: "block break-all font-mono text-2 text-ink", children: body.command }) }), _jsx("div", { className: "mt-1.5 text-2 text-ink-3", children: body.why })] }));
        case 'askWeb':
            return (_jsxs("div", { className: "mt-2 text-2", children: [_jsx("div", { className: "text-ink-2", children: body.why }), _jsx("div", { className: "mt-1 font-mono text-2 text-ink-3", children: body.url })] }));
        case 'artifact':
            return _jsx(ArtifactBody, { body: body });
        case 'memory':
            return (_jsxs("div", { className: "mt-1.5", children: [_jsx("div", { className: "text-3 text-ink", children: body.note }), _jsx("div", { className: "mt-1 text-1 text-ink-3", children: body.noteKind })] }));
        case 'wake':
            return (_jsxs("div", { className: "mt-1.5 text-2 text-ink-2", children: [_jsx("span", { className: "brand-type text-ink", children: body.when }), _jsx("span", { className: "mx-1.5 text-ink-3", children: "\u00B7" }), body.todo] }));
        case 'roleJoin':
        case 'assignRole':
            return (_jsxs("div", { className: "mt-1.5 text-2 text-ink-2", children: [_jsx("span", { className: "text-ink", children: body.role }), _jsx("span", { className: "mx-1.5 text-ink-3", children: "\u00B7" }), body.why] }));
        case 'planChanged':
            return (_jsxs("div", { className: "mt-2 grid gap-1 text-2", children: [_jsxs("div", { className: "text-ink-3", children: ["\u539F\u6765\uFF1A", _jsx("span", { className: "text-ink-2", children: body.before.join(' → ') })] }), _jsxs("div", { className: "text-ink-3", children: ["\u73B0\u5728\uFF1A", _jsx("span", { className: "text-ink", children: body.after.join(' ∥ ') })] }), _jsx("div", { className: "text-ink-3", children: body.why })] }));
        case 'milestone':
            return (_jsx("ul", { className: "mt-2 space-y-0.5 text-2", children: body.criteria.map((item) => (_jsxs("li", { className: "flex gap-1.5", children: [_jsx("span", { className: item.ok ? 'text-done' : 'text-wait', children: item.ok ? '✓' : '◐' }), _jsx("span", { className: "text-ink-2", children: item.text })] }, item.text))) }));
        case 'quotaAlert':
            return (_jsxs("div", { className: "mt-1.5 text-2 text-ink-2", children: [_jsx("div", { className: "text-ink", children: body.line }), _jsx("div", { className: "mt-0.5 brand-type text-ink-3", children: body.used })] }));
        case 'problem':
            return (_jsxs("div", { className: "mt-1.5 text-2 text-ink-2", children: [_jsx("div", { children: body.what }), _jsxs("div", { className: "mt-1 text-1 text-ink-3", children: ["Kind \u7684\u5904\u7406\uFF1A", body.handled] })] }));
        default:
            return null;
    }
}
function DoneLine({ card }) {
    return (_jsxs("div", { className: "mt-1.5 flex items-center gap-1.5 text-2 text-ink-3", children: [_jsx("span", { className: "text-done", children: "\u2713" }), _jsxs("span", { children: [TYPE_LABEL[card.type], " \u00B7 ", card.done?.line] }), _jsx("span", { className: "brand-type opacity-70", children: card.done?.at })] }));
}
export const KindCardView = memo(function KindCardView({ card }) {
    if (card.done)
        return _jsx(DoneLine, { card: card });
    const framed = FRAMED.has(card.type);
    return (_jsxs("div", { className: cn('mt-2.5', framed ? 'rounded-card border border-line-2 bg-raised px-3.5 py-2.5' : 'px-0.5 py-0'), children: [_jsxs("div", { className: "flex min-w-0 items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: cn('shrink-0', TONE[card.type]), children: TYPE_LABEL[card.type] }), _jsx("span", { className: "shrink-0 text-ink-3/60", children: "\u00B7" }), _jsx(Origin, { card: card }), _jsx("span", { className: "ml-auto shrink-0 brand-type", children: card.time })] }), _jsx("div", { className: "mt-1 text-4 font-medium leading-6 text-ink", children: card.title }), card.body ? _jsx(Body, { body: card.body }) : null, _jsxs("div", { className: "mt-2.5 flex flex-wrap items-center gap-1.5", children: [card.actions.map((action) => (_jsx("button", { type: "button", onClick: () => handleAction(card, action.label, action.mutates), className: cn('h-7 rounded-btn px-2.5 text-2 transition-colors', action.primary
                            ? 'bg-ink text-content hover:opacity-90'
                            : 'border border-line-2 text-ink-2 hover:bg-hover hover:text-ink'), children: action.label }, action.label))), card.origin.go && !card.actions.some((action) => action.label.includes('去看')) ? (_jsx("button", { type: "button", onClick: () => go(card.origin.go.replace(/^#/, '')), className: "ml-auto text-2 text-ink-3 transition-colors hover:text-ink", children: "\u53BB\u770B \u203A" })) : null] })] }));
});
