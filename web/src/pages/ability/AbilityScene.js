import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { listAbilities, listBuiltinFlows, listFlowIndex } from '@/data/api';
import { FlowEditor } from './FlowEditor';
import { cn } from '@/lib/utils';
/** 能力场景（v13 的「能力」页 + 第 1 期新增的「流程」分组）。 */
const STATUS_TEXT = {
    ok: '可用',
    needAuth: '需要授权',
    needKey: '缺 API key',
    needDep: '缺依赖',
};
const STATUS_ACTION = {
    ok: null,
    needAuth: '去授权',
    needKey: '填写 key',
    needDep: '安装依赖',
};
const STATUS_TONE = {
    ok: 'bg-done',
    needAuth: 'bg-wait',
    needKey: 'bg-wait',
    needDep: 'bg-wait',
};
function AbilityRow({ ability }) {
    const action = STATUS_ACTION[ability.status];
    return (_jsxs("div", { className: "flex items-center gap-3 border-b border-line px-4 py-2.5 last:border-b-0", children: [_jsxs("div", { className: "min-w-0 flex-1", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsx("span", { className: "truncate text-3 text-ink", children: ability.name }), _jsx("span", { className: "shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3", children: ability.type }), _jsx("span", { className: "shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3", children: ability.source })] }), _jsxs("div", { className: "mt-0.5 truncate text-1 text-ink-3", children: ["\u914D\u5907\uFF1A", ability.equippedBy.join('、')] })] }), _jsxs("div", { className: "flex shrink-0 items-center gap-2 text-2 text-ink-3", children: [_jsx("i", { className: cn('size-1.5 rounded-full', STATUS_TONE[ability.status]) }), STATUS_TEXT[ability.status], action ? (_jsx("button", { type: "button", title: "\u7B2C 3 \u671F", className: "rounded-btn border border-line-2 px-2 py-[2px] text-2 text-ink-2", children: action })) : null] })] }));
}
function FlowRow({ flow }) {
    return (_jsxs("button", { type: "button", onClick: () => go('flow:' + flow.id), className: "flex w-full items-center gap-3 border-b border-line px-4 py-2.5 text-left transition-colors last:border-b-0 hover:bg-hover", children: [_jsxs("div", { className: "min-w-0 flex-1", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsx("span", { className: "truncate text-3 text-ink", children: flow.name }), _jsx("span", { className: "shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3", children: flow.version }), _jsx("span", { className: "shrink-0 rounded-badge border border-line px-1 py-[1px] text-1 text-ink-3", children: flow.source })] }), _jsxs("div", { className: "mt-0.5 truncate text-1 text-ink-3", children: ["\u7528\u8FC7 ", flow.usedTimes, " \u6B21", flow.projectName ? ' · 最近用在 ' + flow.projectName : ''] })] }), _jsxs("span", { className: "flex shrink-0 items-center gap-1 text-2 text-ink-3", children: ["\u6253\u5F00", _jsx(ChevronRight, { className: "size-3.5" })] })] }));
}
function BuiltinFlowCard({ flow }) {
    return (_jsxs("div", { className: "rounded-card border border-line bg-pane/40 p-3", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsx("span", { className: "rounded-full bg-hover px-2 py-[1px] text-1 text-ink-2", children: flow.tier }), _jsx("span", { className: "truncate text-3 text-ink", children: flow.name })] }), _jsx("p", { className: "mt-1 text-1 leading-4 text-ink-3", children: flow.description }), _jsx("div", { className: "mt-1.5 flex flex-wrap items-center gap-1 text-1 text-ink-2", children: flow.steps.map((step, index) => (_jsxs("span", { className: "flex items-center gap-1", children: [_jsx("span", { children: step }), index < flow.steps.length - 1 ? _jsx("span", { className: "text-ink-3", children: "\u2192" }) : null] }, step))) }), _jsxs("div", { className: "mt-2 flex items-center gap-2", children: [_jsx("span", { className: "rounded-btn border border-line px-2 py-[2px] text-1 text-ink-3", title: "\u5185\u7F6E\u6D41\u7A0B\u53EA\u8BFB", children: "\u53EA\u8BFB" }), _jsx("button", { type: "button", onClick: () => toast('（原型）已复制成你的流程；副本会出现在上面的「流程」里'), className: "rounded-btn border border-line-2 px-2 py-[2px] text-1 text-ink-2 transition-colors hover:bg-hover", children: "\u590D\u5236\u6210\u81EA\u5DF1\u7684" })] })] }));
}
function AbilityLibrary() {
    const [abilities, setAbilities] = useState(null);
    const [flows, setFlows] = useState(null);
    const [builtin, setBuiltin] = useState(null);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const [abilityList, flowList, builtinList] = await Promise.all([listAbilities(), listFlowIndex(), listBuiltinFlows()]);
            if (!alive)
                return;
            setAbilities(abilityList);
            setFlows(flowList);
            setBuiltin(builtinList);
        })();
        return () => {
            alive = false;
        };
    }, []);
    return (_jsx("div", { className: "h-full overflow-y-auto", children: _jsxs("div", { className: "mx-auto w-full max-w-[1000px] px-10 py-8", children: [_jsx("h1", { className: "text-6 font-medium leading-8 text-ink", children: "\u80FD\u529B" }), _jsx("p", { className: "mt-2 max-w-[620px] text-3 leading-5 text-ink-2", children: "\u6280\u80FD\u3001\u5DE5\u5177\u3001\u8FDE\u63A5\u4E0E\u7EC4\u5408\u5DE5\u5177\u90FD\u5728\u8FD9\u91CC\u3002\u89D2\u8272\u53EF\u4EE5\u914D\u5907\u80FD\u529B\uFF1B\u9700\u8981\u5904\u7406\u7684\u72B6\u6001\u65C1\u8FB9\u90FD\u6709\u6309\u94AE\u3002" }), _jsxs("section", { className: "mt-4", "data-builtin-flows": "true", children: [_jsxs("h2", { className: "flex items-center gap-2 text-3 font-medium text-ink", children: [_jsx(ChevronRight, { className: "size-3.5 rotate-90 text-ink-3" }), "\u5185\u7F6E\u6D41\u7A0B", _jsx("span", { className: "brand-type text-2 font-normal text-ink-3", children: builtin ? builtin.length : 0 }), _jsx("span", { className: "ml-1 text-1 font-normal text-ink-3", children: "\u4E09\u6863\uFF1A\u8F7B\u6863 / \u666E\u901A\u6863 / \u914D\u5BF9\u6863 \u00B7 \u53EA\u8BFB\uFF0C\u53EF\u590D\u5236\u6210\u81EA\u5DF1\u7684\uFF1B\u65B0\u5EFA\u6D41\u7A0B \u2192 \u6A21\u677F\u91CC\u4E5F\u6709" })] }), _jsx("div", { className: "mt-2 grid gap-2 sm:grid-cols-3", children: (builtin ?? []).map((flow) => (_jsx(BuiltinFlowCard, { flow: flow }, flow.id))) })] }), _jsxs("section", { className: "mt-4", "data-flow-group": "true", children: [_jsxs("h2", { className: "flex items-center gap-2 text-3 font-medium text-ink", children: [_jsx(ChevronRight, { className: "size-3.5 rotate-90 text-ink-3" }), "\u6D41\u7A0B", _jsx("span", { className: "brand-type text-2 font-normal text-ink-3", children: flows ? flows.length : 0 }), _jsx("span", { className: "ml-1 text-1 font-normal text-ink-3", children: "\u4E00\u7C7B\u4E8B\u56FA\u5B9A\u600E\u4E48\u505A\uFF1A\u8C01\u505A\u54EA\u4E00\u6B65\u3001\u54EA\u91CC\u8BBE\u95E8\u3001\u6BCF\u7AD9\u4EA4\u51FA\u4EC0\u4E48 \u00B7 \u70B9\u4E00\u6761\u6253\u5F00\u7F16\u8F91\u5668" })] }), _jsxs("div", { className: "mt-2 overflow-hidden rounded-card border border-line bg-pane/40", children: [(flows ?? []).map((flow) => (_jsx(FlowRow, { flow: flow }, flow.id))), flows && flows.length === 0 ? _jsx("div", { className: "px-4 py-3 text-2 text-ink-3", children: "\u8FD8\u6CA1\u6709\u6D41\u7A0B\u3002" }) : null] })] }), abilityGroupsOf(abilities).map((group) => (_jsxs("section", { className: "mt-6", children: [_jsxs("h2", { className: "flex items-center gap-2 text-3 font-medium text-ink", children: [_jsx(ChevronRight, { className: "size-3.5 rotate-90 text-ink-3" }), group.name, _jsx("span", { className: "brand-type text-2 font-normal text-ink-3", children: group.items.length })] }), _jsxs("div", { className: "mt-2 overflow-hidden rounded-card border border-line bg-pane/40", children: [group.items.map((ability) => (_jsx(AbilityRow, { ability: ability }, ability.id))), group.items.length === 0 ? _jsx("div", { className: "px-4 py-3 text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u2026" }) : null] })] }, group.key))), _jsx("p", { className: "mt-8 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u80FD\u529B\u5E93\uFF1A\u80FD\u529B\u662F\u89D2\u8272\u80FD\u7528\u7684\u5DE5\u5177 / \u6280\u80FD / \u8FDE\u63A5\uFF1B\u6D41\u7A0B\u662F\u300C\u4E00\u7C7B\u4E8B\u56FA\u5B9A\u600E\u4E48\u505A\u300D\u7684\u53EF\u590D\u7528\u6A21\u677F\uFF0C\u5185\u7F6E\u6D41\u7A0B\u53EA\u8BFB\u3001\u53EF\u590D\u5236\u3002" })] }) }));
}
function abilityGroupsOf(abilities) {
    const list = abilities ?? [];
    return [
        { key: 'daily', name: '日常', items: list.filter((ability) => ability.group === 'daily') },
        { key: 'pro', name: '专业', items: list.filter((ability) => ability.group === 'pro') },
    ];
}
export function AbilityScene() {
    const route = useRouteStore((state) => state.route);
    if (route.kind === 'flow')
        return _jsx(FlowEditor, { flowId: route.id }, route.id);
    return _jsx(AbilityLibrary, {});
}
