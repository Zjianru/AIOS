import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useState } from 'react';
import { ChevronDown, Sparkles, X } from 'lucide-react';
import { JOIN_OPTIONS, NODE_KIND_LABEL, ROLE_OPTIONS } from '@/data/flows';
import { cn } from '@/lib/utils';
import { isShared } from './flowGraph';
const fieldBase = 'w-full rounded-btn border border-line-2 bg-raised px-2 text-2 text-ink outline-none transition-colors placeholder:text-ink-3 focus:border-run disabled:cursor-not-allowed disabled:opacity-60';
const labelBase = 'mb-1 mt-3 block text-1 font-semibold text-ink-3';
const selectBase = 'h-7 w-full rounded-btn border border-line-2 bg-raised px-1.5 text-2 text-ink outline-none focus:border-run disabled:cursor-not-allowed disabled:opacity-60';
const KIND_SEGMENTS = [
    { kind: 'station', label: '站' },
    { kind: 'stack', label: '逐条展开' },
    { kind: 'gate', label: '门' },
    { kind: 'pool', label: '池' },
];
const DIFF_FIELD_LABEL = {
    name: '名称',
    who: '谁来做',
    give: '交出',
    recv: '收到',
    dod: '完成',
    rhythm: '节奏',
    members: '成员',
    user: '等你',
    ext: '对外',
    loop: '循环',
    rule: '算过',
    fail: '没过',
    note: '说明',
    fan: '逐条展开',
    join: '怎么汇合',
};
function lineBadge(flow, lineId) {
    const line = flow.lines.find((item) => item.id === lineId);
    if (!line)
        return null;
    return (_jsx("span", { className: "inline-grid size-[15px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-bold leading-none text-white", style: { background: line.color }, title: line.name, children: line.index }, lineId));
}
function versionOf(flow) {
    return flow.draft ? '草稿' : 'v' + flow.version;
}
function inUse(flow) {
    return flow.instances.filter((instance) => instance.state !== 'done').length;
}
function showValue(value) {
    if (Array.isArray(value)) {
        return value
            .map((item) => {
            if (Array.isArray(item))
                return String(item[0] ?? '');
            if (item && typeof item === 'object' && 'text' in item)
                return String(item.text);
            return String(item);
        })
            .join('、');
    }
    if (value && typeof value === 'object') {
        const loop = value;
        return '上限 ' + (loop.max ?? '?') + ' 次' + (loop.until ? '，直到' + loop.until : '');
    }
    if (value === true)
        return '是';
    if (value === false)
        return '否';
    if (value === null || value === undefined || value === '')
        return '（空）';
    return String(value);
}
function InputRow(props) {
    const { label, value, placeholder, multiline, bad, disabled, onChange } = props;
    return (_jsxs("label", { className: "block", children: [_jsx("span", { className: labelBase, children: label }), multiline ? (_jsx("textarea", { className: cn(fieldBase, 'min-h-[50px] resize-y py-1 leading-5', bad && 'border-fail'), rows: 2, value: value, placeholder: placeholder, disabled: disabled, onChange: (event) => onChange(event.target.value) })) : (_jsx("input", { className: cn(fieldBase, 'h-7', bad && 'border-fail'), value: value, placeholder: placeholder, disabled: disabled, onChange: (event) => onChange(event.target.value) }))] }));
}
function TagList(props) {
    const { values, disabled, onRemove } = props;
    if (!values.length)
        return _jsx("span", { className: "text-2 text-ink-3", children: "\uFF08\u7A7A\uFF09" });
    return (_jsx("span", { className: "flex flex-wrap items-center gap-1", children: values.map((value, index) => (_jsxs("span", { className: "inline-flex h-6 items-center gap-0.5 rounded-btn bg-hover pl-2 pr-0.5 text-2 text-ink", children: [value, _jsx("button", { type: "button", onClick: () => onRemove(index), disabled: disabled, className: "grid size-4 place-items-center rounded-[3px] text-ink-3 hover:bg-line-2 hover:text-ink disabled:cursor-not-allowed disabled:opacity-50", "aria-label": '去掉' + value, children: _jsx(X, { className: "size-2.5" }) })] }, value + index))) }));
}
function RoleSelect(props) {
    const { have, keyName, disabled, onAdd } = props;
    return (_jsxs("select", { className: "e-add h-6 rounded-btn border border-dashed border-line-2 bg-transparent px-1 text-1 text-ink-3 outline-none hover:text-ink focus:border-run", value: "", disabled: disabled, "aria-label": '加一位' + keyName, onChange: (event) => {
            const value = event.target.value;
            event.currentTarget.value = '';
            if (value)
                onAdd(value);
        }, children: [_jsx("option", { value: "", children: "\uFF0B \u52A0" }), ROLE_OPTIONS.filter((role) => !have.includes(role)).map((role) => (_jsx("option", { value: role, children: role }, role)))] }));
}
function VersionList({ flow }) {
    const byVersion = new Map();
    for (const instance of flow.instances) {
        if (instance.state === 'done')
            continue;
        byVersion.set(instance.version, (byVersion.get(instance.version) ?? 0) + 1);
    }
    if (!flow.versions.length)
        return _jsx("p", { className: "text-2 text-ink-3", children: "\u8FD8\u6CA1\u4FDD\u5B58\u8FC7\u3002" });
    return (_jsx("ul", { className: "flex flex-col gap-0.5", children: flow.versions.map((version) => (_jsxs("li", { className: "grid grid-cols-[34px_36px_1fr] items-baseline gap-1.5 py-0.5 text-2 text-ink-2", children: [_jsxs("b", { className: "brand-type text-2 font-bold text-ink", children: ["v", version.version] }), _jsx("span", { className: "brand-type text-1 text-ink-3", children: version.at }), _jsxs("span", { className: "min-w-0", children: [version.note, byVersion.get(version.version) ? (_jsxs("span", { className: "ml-1.5 text-1 text-ink-3", children: ["\u00B7 \u5728\u7528 ", byVersion.get(version.version)] })) : null] })] }, version.version))) }));
}
function Overview({ flow, preview, readOnly, instance, onPatchFlowName }) {
    const locked = preview || readOnly;
    return (_jsxs("div", { children: [_jsxs("div", { className: "flex items-center gap-1", children: [_jsx("span", { className: "text-1 text-ink-3", children: "\u6D41\u7A0B" }), _jsx("span", { className: "ml-auto rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2", children: versionOf(flow) })] }), _jsx("input", { className: "-mx-1 mt-1 w-full rounded-btn border border-transparent bg-transparent px-1 py-0.5 text-5 font-semibold text-ink outline-none hover:border-line-2 focus:border-run focus:bg-raised disabled:cursor-not-allowed", value: flow.name, "aria-label": "\u6D41\u7A0B\u540D", disabled: locked, onChange: (event) => onPatchFlowName(event.target.value) }), _jsx("p", { className: "mt-2 text-2 text-ink-3", children: flow.source }), instance ? (_jsxs("div", { className: "mt-3 rounded-card border border-line bg-pane px-2.5 py-2", children: [_jsxs("div", { className: "flex items-center gap-1.5", children: [_jsx("span", { className: cn('size-1.5 rounded-full', instance.state === 'run' ? 'bg-run' : instance.state === 'wait' ? 'bg-wait' : 'bg-done') }), _jsx("span", { className: "text-2 font-medium text-ink", children: instance.name }), _jsxs("span", { className: "ml-auto rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2", children: ["v", instance.version] })] }), _jsxs("div", { className: "mt-1 text-2 text-ink-2", children: [instance.stateText, instance.project ? ' · ' + instance.project.name : ''] }), _jsx("div", { className: "mt-1 text-1 text-ink-3", children: instance.overrides.length ? '本地改了 ' + instance.overrides.length + ' 处' : '照流程跑，没改' })] })) : null, preview ? (_jsx("div", { className: "mt-3 rounded-card border border-run/40 bg-run/[0.06] px-2.5 py-2 text-2 text-ink-2", children: "\u6B63\u5728\u770B\u4FEE\u6539\u63D0\u8BAE\uFF1A\u7EFF = \u65B0\u52A0\uFF0C\u7EA2 = \u53BB\u6389\uFF0C\u84DD = \u6539\u4E86\u3002\u63A5\u53D7\u6216\u4E0D\u8981\u4E4B\u540E\u518D\u6539\u3002" })) : readOnly ? (_jsx("div", { className: "mt-3 rounded-card border border-line bg-pane px-2.5 py-2 text-2 text-ink-2", children: "\u8FD9\u4E2A\u6D41\u7A0B\u73B0\u5728\u53EA\u8BFB\uFF1B\u56DE\u5230\u300C\u6D41\u7A0B\u300D\u6216\u6062\u590D\u540E\u518D\u6539\u3002" })) : (_jsxs("div", { className: "mt-3 space-y-1 text-2 text-ink-2", children: [_jsx("p", { children: "\u62D6\u96F6\u4EF6\u5230\u7EBF\u4E0A\uFF0C\u80FD\u653E\u7684\u4F4D\u7F6E\u4F1A\u4EAE\uFF1B\u70B9\u4E00\u7AD9\u6539\u5B83\uFF0C\u4E5F\u53EF\u4EE5\u6307\u7740\u4E00\u7AD9\u8DDF Kind \u8BF4\u3002" }), _jsx("p", { children: "\u4FDD\u5B58\u65F6\u5148\u68C0\u67E5\uFF1A\u6709\u8981\u8865\u7684\u5C31\u5B58\u4E0D\u4E86\uFF0C\u6BCF\u6761\u90FD\u6709\u4E00\u952E\u8865\u6CD5\u3002" })] })), _jsx("div", { className: labelBase, children: "\u4EF7\u503C\u6D41" }), _jsx("div", { className: "flex flex-col gap-1", children: flow.lines.map((line) => (_jsxs("div", { className: "flex items-center gap-1.5 text-2 text-ink-2", children: [_jsx("span", { className: "inline-grid size-[15px] place-items-center rounded-badge brand-type text-[9px] font-bold text-white", style: { background: line.color }, children: line.index }), _jsx("span", { className: "text-ink", children: line.name }), _jsx("span", { className: "ml-auto text-1 text-ink-3", children: line.what })] }, line.id))) }), _jsx("div", { className: labelBase, children: "\u7248\u672C" }), _jsx(VersionList, { flow: flow }), _jsxs("div", { className: "mt-3 text-1 leading-4 text-ink-3", children: [inUse(flow), " \u4E2A\u5B9E\u4F8B\u5728\u7528 \u00B7 \u7248\u672C\u56FA\u5B9A\uFF0C\u5347\u7EA7\u8981\u9010\u4E2A\u70B9"] })] }));
}
function DiffDetail({ flow, node }) {
    const diff = node.diff;
    const shared = isShared(flow, node);
    return (_jsxs("div", { children: [_jsxs("div", { className: "flex items-center gap-1", children: [_jsxs("span", { className: "text-1 text-ink-3", children: [NODE_KIND_LABEL[node.kind], shared ? '  多条线共用' : ''] }), _jsx("span", { className: "ml-auto text-1 text-ink-3", children: "\u53EA\u8BFB" })] }), _jsx("div", { className: "mt-1 text-5 font-semibold text-ink", children: node.name }), diff === 'add' ? (_jsx("div", { className: "mt-2 rounded-card border border-done/40 bg-done/[0.06] px-2.5 py-2 text-2 text-ink-2", children: _jsx("b", { className: "text-done", children: "\u65B0\u52A0\u7684\u4E00\u7AD9" }) })) : null, diff === 'del' ? (_jsxs("div", { className: "mt-2 rounded-card border border-fail/40 bg-fail/[0.06] px-2.5 py-2 text-2 text-ink-2", children: [_jsx("b", { className: "text-fail", children: "\u8FD9\u4E00\u7AD9\u4F1A\u53BB\u6389" }), "\uFF0C\u524D\u540E\u4E24\u7AD9\u76F4\u63A5\u63A5\u4E0A\u3002"] })) : null, diff === 'chg' ? (_jsxs("div", { className: "mt-2 rounded-card border border-run/40 bg-run/[0.06] px-2.5 py-2 text-2", children: [_jsx("b", { className: "text-run", children: "\u6539\u4E86\u8FD9\u4E9B" }), _jsx("dl", { className: "mt-1 grid grid-cols-[44px_1fr] gap-x-2 gap-y-1 text-2", children: Object.entries(node.diffOld ?? {}).map(([key, oldValue]) => {
                            let next = '';
                            if (key === 'name')
                                next = node.name;
                            else if (key === 'who')
                                next = node.who ?? [];
                            else if (key === 'give')
                                next = node.give ?? '';
                            else if (key === 'recv')
                                next = node.recv ?? '';
                            else if (key === 'dod')
                                next = node.dod ?? [];
                            else if (key === 'rhythm')
                                next = node.rhythm ?? '';
                            else if (key === 'members')
                                next = node.members ?? [];
                            else if (key === 'rule')
                                next = node.rule ?? '';
                            else if (key === 'fail')
                                next = node.fail ?? '';
                            else if (key === 'loop')
                                next = node.loop ?? null;
                            else if (key === 'user')
                                next = node.user ?? false;
                            else
                                next = '';
                            return (_jsxs("div", { className: "contents", children: [_jsx("dt", { className: "text-ink-3", children: DIFF_FIELD_LABEL[key] ?? key }), _jsxs("dd", { className: "min-w-0 text-ink-2", children: [_jsx("s", { className: "text-fail", children: showValue(oldValue) }), _jsx("br", {}), _jsx("span", { className: "text-run", children: showValue(next) })] })] }, key));
                        }) })] })) : null, _jsx("div", { className: labelBase, children: "\u4EA4\u63A5" }), _jsxs("dl", { className: "grid grid-cols-[34px_1fr] gap-x-2 gap-y-1 text-2", children: [_jsx("dt", { className: "text-ink-3", children: "\u6536\u5230" }), _jsx("dd", { className: "text-ink-2", children: node.recv || '（空）' }), _jsx("dt", { className: "text-ink-3", children: "\u4EA4\u51FA" }), _jsx("dd", { className: "text-ink-2", children: node.give || '（空）' })] }), node.dod?.length ? (_jsxs(_Fragment, { children: [_jsx("div", { className: labelBase, children: "\u5B8C\u6210\u6807\u51C6" }), _jsx("ul", { className: "space-y-0.5 text-2 text-ink-2", children: node.dod.map((item) => (_jsx("li", { children: item.text }, item.text))) })] })) : null, _jsx("p", { className: "mt-4 text-1 text-ink-3", children: "\u770B\u4FEE\u6539\u65F6\u4E0D\u80FD\u7F16\u8F91\uFF1B\u63A5\u53D7\u6216\u4E0D\u8981\u4E4B\u540E\u518D\u6539\u3002" })] }));
}
function Detail(props) {
    const { flow, node, readOnly, archived } = props;
    const [skillDraft, setSkillDraft] = useState('');
    const shared = isShared(flow, node);
    const kindLabel = NODE_KIND_LABEL[node.kind];
    const supportsType = node.kind === 'station' || node.kind === 'stack' || node.kind === 'gate' || node.kind === 'pool';
    const showHandoff = node.kind === 'pool' || node.kind === 'gate' || node.kind === 'station' || node.kind === 'stack';
    const showWorks = node.kind === 'gate' || node.kind === 'station' || node.kind === 'stack';
    const patch = (value) => props.onPatchNode(node.id, value);
    const addSkill = () => {
        const value = skillDraft.trim();
        if (!value)
            return;
        props.onAddToArray(node.id, 'skills', value);
        setSkillDraft('');
    };
    return (_jsxs("div", { children: [_jsxs("div", { className: "flex items-center gap-1", children: [_jsxs("span", { className: "text-1 text-ink-3", children: [kindLabel, shared ? '  多条线共用' : ''] }), !readOnly ? (_jsxs("button", { type: "button", onClick: props.onPointToKind, className: "ml-auto inline-flex h-6 items-center gap-1 rounded-btn px-1.5 text-1 text-run hover:bg-run/10", children: [_jsx(Sparkles, { className: "size-3" }), "\u6307\u7ED9 Kind"] })) : null, _jsx("button", { type: "button", onClick: props.onClose, className: cn('grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink', readOnly ? 'ml-auto' : ''), "aria-label": "\u5173\u4E0A\u8BE6\u60C5", children: _jsx(X, { className: "size-4" }) })] }), readOnly ? (_jsx("p", { className: "mt-1 rounded-btn bg-hover px-2 py-1 text-1 text-ink-3", children: archived ? '已归档只读；恢复后才能改。' : '实例态只读；回到「流程」再改。' })) : null, _jsx("input", { className: "-mx-1 mt-1 w-full rounded-btn border border-transparent bg-transparent px-1 py-0.5 text-5 font-semibold text-ink outline-none hover:border-line-2 focus:border-run focus:bg-raised disabled:cursor-not-allowed", value: node.name, "aria-label": "\u8FD9\u4E00\u7AD9\u7684\u540D\u5B57", disabled: readOnly, onChange: (event) => patch({ name: event.target.value }) }), _jsxs("div", { className: "mt-1 flex flex-wrap items-center gap-1.5", children: [node.lines.map((lineId) => lineBadge(flow, lineId)), node.user ? _jsx("span", { className: "rounded-badge bg-wait-soft px-1.5 py-0.5 text-1 font-medium text-wait", children: "\u6BCF\u6B21\u7B49\u4F60" }) : null, node.ext ? _jsx("span", { className: "rounded-badge bg-fail-soft px-1.5 py-0.5 text-1 font-medium text-fail", children: "\u5BF9\u5916" }) : null, node.loop ? _jsxs("span", { className: "rounded-badge bg-hover px-1.5 py-0.5 brand-type text-1 text-ink-2", children: ["\u2264", node.loop.max ?? '?', " \u6B21"] }) : null] }), supportsType ? (_jsx("fieldset", { className: "mt-2 flex rounded-btn bg-hover p-0.5", disabled: readOnly, children: KIND_SEGMENTS.map((segment) => (_jsx("button", { type: "button", onClick: () => props.onSetNodeKind(node.id, segment.kind), "aria-pressed": node.kind === segment.kind, className: cn('h-6 flex-1 rounded-[4px] text-1 text-ink-2 transition-colors disabled:cursor-not-allowed', node.kind === segment.kind ? 'bg-ink font-semibold text-content' : 'hover:text-ink'), children: segment.label }, segment.kind))) })) : null, node.kind === 'pool' ? (_jsx(InputRow, { label: "\u8BF4\u660E", value: node.note ?? '', multiline: true, disabled: readOnly, onChange: (value) => patch({ note: value }) })) : null, node.kind === 'gate' ? (_jsxs(_Fragment, { children: [_jsx("div", { className: labelBase, children: "\u95E8\u91CC\u7684\u4EBA" }), _jsxs("div", { className: "flex flex-wrap items-center gap-1", children: [_jsx(TagList, { values: node.members ?? [], disabled: readOnly, onRemove: (index) => props.onRemoveFromArray(node.id, 'members', index) }), !readOnly ? (_jsx(RoleSelect, { have: node.members ?? [], keyName: "\u5BA1\u8BFB", onAdd: (value) => props.onAddToArray(node.id, 'members', value) })) : null] }), _jsxs("label", { className: "block", children: [_jsx("span", { className: labelBase, children: "\u600E\u4E48\u7B97\u8FC7" }), _jsx("select", { className: selectBase, value: node.rule ?? '全部通过', disabled: readOnly, onChange: (event) => patch({ rule: event.target.value }), children: ['全部通过', '一票否决', '多数通过'].map((rule) => (_jsx("option", { value: rule, children: rule }, rule))) })] }), _jsx(InputRow, { label: "\u6CA1\u8FC7\u600E\u4E48\u529E", value: node.fail ?? '', multiline: true, disabled: readOnly, onChange: (value) => patch({ fail: value }) })] })) : null, node.kind === 'station' || node.kind === 'stack' ? (_jsxs(_Fragment, { children: [_jsx("div", { className: labelBase, children: "\u8C01\u6765\u505A" }), _jsxs("div", { className: "flex flex-wrap items-center gap-1", children: [_jsx(TagList, { values: node.who ?? [], disabled: readOnly, onRemove: (index) => props.onRemoveFromArray(node.id, 'who', index) }), !readOnly ? (_jsx(RoleSelect, { have: node.who ?? [], keyName: "\u8C01\u6765", onAdd: (value) => props.onAddToArray(node.id, 'who', value) })) : null] }), _jsx("div", { className: labelBase, children: "\u7528\u5230\u7684\u6280\u80FD" }), _jsx("div", { className: "flex flex-wrap items-center gap-1", children: _jsx(TagList, { values: node.skills ?? [], disabled: readOnly, onRemove: (index) => props.onRemoveFromArray(node.id, 'skills', index) }) }), !readOnly ? (_jsx("input", { className: cn(fieldBase, 'mt-1.5 h-7'), value: skillDraft, placeholder: "\u52A0\u4E00\u4E2A\u6280\u80FD\uFF0C\u56DE\u8F66", onChange: (event) => setSkillDraft(event.target.value), onKeyDown: (event) => {
                            if (event.key === 'Enter') {
                                event.preventDefault();
                                addSkill();
                            }
                        } })) : null] })) : null, node.kind === 'stack' ? (_jsx(InputRow, { label: "\u9010\u6761\u5C55\u5F00", value: node.fan ?? '', placeholder: "\u6BD4\u5982\uFF1A\u6BCF\u6761\u5199\u8349\u7A3F", disabled: readOnly, onChange: (value) => patch({ fan: value }) })) : null, showHandoff ? (_jsxs(_Fragment, { children: [_jsx("div", { className: labelBase, children: "\u4EA4\u63A5" }), _jsxs("div", { className: "grid grid-cols-[34px_1fr] items-start gap-x-2 gap-y-1", children: [_jsx("span", { className: "pt-1.5 text-2 text-ink-3", children: "\u6536\u5230" }), _jsx("textarea", { className: cn(fieldBase, 'min-h-[42px] resize-y py-1 leading-5'), value: node.recv ?? '', placeholder: "\u4ECE\u4E0A\u4E00\u7AD9\u62FF\u5230\u4EC0\u4E48", rows: 2, disabled: readOnly, onChange: (event) => patch({ recv: event.target.value }) }), _jsx("span", { className: "pt-1.5 text-2 text-ink-3", children: "\u4EA4\u51FA" }), _jsx("textarea", { className: cn(fieldBase, 'min-h-[42px] resize-y py-1 leading-5', showWorks && !(node.give ?? '').trim() && 'border-fail'), value: node.give ?? '', placeholder: node.kind === 'pool' ? '下游来取时给什么' : '交给下一站什么', rows: 2, disabled: readOnly, onChange: (event) => patch({ give: event.target.value }) })] })] })) : null, showWorks ? (_jsxs(_Fragment, { children: [_jsx("div", { className: labelBase, children: "\u5B8C\u6210\u6807\u51C6" }), _jsx("div", { className: "space-y-1", children: (node.dod ?? []).map((item, index) => (_jsxs("div", { className: "flex items-center gap-1", children: [_jsx("input", { className: cn(fieldBase, 'h-7 min-w-0 flex-1'), value: item.text, disabled: readOnly, onChange: (event) => props.onPatchDod(node.id, index, { text: event.target.value }) }), _jsx("button", { type: "button", onClick: () => props.onPatchDod(node.id, index, { auto: !item.auto }), disabled: readOnly, className: cn('h-6 shrink-0 rounded-[3px] px-1 text-[10px]', item.auto
                                        ? 'text-done ring-1 ring-done/40'
                                        : 'text-ink-3 ring-1 ring-line-2'), children: item.auto ? '程序可查' : '要人看' }), _jsx("button", { type: "button", onClick: () => props.onRemoveDod(node.id, index), disabled: readOnly, className: "grid size-5 shrink-0 place-items-center rounded-[3px] text-ink-3 hover:bg-hover hover:text-ink disabled:cursor-not-allowed", "aria-label": "\u53BB\u6389\u8FD9\u6761\u5B8C\u6210\u6807\u51C6", children: _jsx(X, { className: "size-3" }) })] }, index))) }), !readOnly ? (_jsx("button", { type: "button", onClick: () => props.onAddDod(node.id), className: "mt-1 h-6 text-1 text-run hover:underline", children: "\uFF0B \u52A0\u4E00\u6761" })) : null, _jsx(InputRow, { label: "\u8282\u594F", value: node.rhythm ?? '', placeholder: "\u53EF\u7A7A\uFF1B\u6BD4\u5982\uFF1A\u6BCF\u5929 10:00 \u4ECE\u6C60\u91CC\u53D6", disabled: readOnly, onChange: (value) => patch({ rhythm: value }) }), _jsx("div", { className: labelBase, children: "\u89C4\u77E9" }), _jsxs("div", { className: "space-y-1 text-2", children: [_jsxs("label", { className: "flex items-center gap-2", children: [_jsx("input", { type: "checkbox", className: "size-3.5 accent-[var(--run)]", checked: Boolean(node.user), disabled: readOnly, onChange: (event) => patch({ user: event.target.checked }) }), "\u6BCF\u6B21\u7B49\u4F60\u70B9\u5934"] }), _jsxs("label", { className: "flex items-center gap-2", children: [_jsx("input", { type: "checkbox", className: "size-3.5 accent-[var(--fail)]", checked: Boolean(node.ext), disabled: readOnly, onChange: (event) => patch({ ext: event.target.checked }) }), "\u5BF9\u5916\uFF0C\u53D1\u51FA\u53BB\u6536\u4E0D\u56DE", _jsx("span", { className: "text-1 text-ink-3", children: "\u52FE\u4E0A\u5C31\u5FC5\u987B\u6BCF\u6B21\u7B49\u4F60" })] }), _jsxs("label", { className: "flex items-center gap-2", children: [_jsx("input", { type: "checkbox", className: "size-3.5 accent-[var(--done)]", checked: Boolean(node.loop), disabled: readOnly, onChange: (event) => patch({ loop: event.target.checked ? { max: 3, until: '' } : undefined }) }), "\u5FAA\u73AF\u5230\u6EE1\u610F"] }), node.loop ? (_jsxs("div", { className: "ml-5 flex items-center gap-1 text-2 text-ink-2", children: ["\u4E0A\u9650", _jsx("input", { type: "number", min: 1, max: 9, className: cn(fieldBase, 'h-6 w-12 px-1'), value: node.loop.max ?? '', disabled: readOnly, onChange: (event) => patch({
                                            loop: {
                                                max: event.target.value ? Number(event.target.value) : null,
                                                until: node.loop?.until ?? '',
                                            },
                                        }) }), "\u6B21\uFF0C\u76F4\u5230", _jsx("input", { className: cn(fieldBase, 'h-6 min-w-0 flex-1 px-1'), value: node.loop.until, placeholder: "\u4E0B\u4E00\u9053\u95E8\u8FC7\u4E86", disabled: readOnly, onChange: (event) => patch({ loop: { max: node.loop?.max ?? null, until: event.target.value } }) })] })) : null] })] })) : null, node.kind === 'join' ? (_jsxs(_Fragment, { children: [_jsxs("label", { className: "block", children: [_jsx("span", { className: labelBase, children: "\u600E\u4E48\u6C47\u5408" }), _jsxs("select", { className: selectBase, value: node.join ?? '', disabled: readOnly, onChange: (event) => patch({ join: (event.target.value || null) }), children: [_jsx("option", { value: "", children: "\u8FD8\u6CA1\u5B9A" }), JOIN_OPTIONS.map((option) => (_jsx("option", { value: option.value, children: option.label }, option.value)))] })] }), _jsx("div", { className: labelBase, children: "\u4ECE\u8FD9\u91CC\u8FDB\u6765\u7684" }), _jsx("div", { className: "text-2 text-ink-2", children: flow.edges
                            .filter((edge) => edge.target === node.id)
                            .map((edge) => flow.nodes.find((item) => item.id === edge.source)?.name ?? '？')
                            .join(' → ') || '还没有线接进来' }), _jsx("p", { className: "mt-2 text-1 leading-4 text-ink-3", children: "\u8981\u4ECE\u4E0A\u4E00\u7AD9\u62C9\u4E00\u6761\u7EBF\u5230\u6C47\u5408\u7684\u5DE6\u8FB9\u628A\u624B\uFF1B\u600E\u4E48\u6C47\u5408\u5199\u6E05\u5168\u90E8\u3001\u4EFB\u4E00\u6216\u591A\u6570\u3002" })] })) : null, node.kind === 'start' ? (_jsxs(_Fragment, { children: [_jsx("div", { className: labelBase, children: "\u600E\u4E48\u5F00\u4E00\u8F6E" }), _jsxs("div", { className: "flex rounded-btn bg-hover p-0.5", children: [_jsx("button", { type: "button", onClick: () => patch({ start: 'manual' }), disabled: readOnly, "aria-pressed": node.start === 'manual', className: cn('h-6 flex-1 rounded-[4px] text-1', node.start === 'manual' ? 'bg-ink font-semibold text-content' : 'text-ink-2'), children: "\u4F60\u8BF4\u5F00\u59CB" }), _jsx("button", { type: "button", onClick: () => patch({ start: 'cron', rhythm: node.rhythm || '每天 9:00' }), disabled: readOnly, "aria-pressed": node.start === 'cron', className: cn('h-6 flex-1 rounded-[4px] text-1', node.start === 'cron' ? 'bg-ink font-semibold text-content' : 'text-ink-2'), children: "\u5230\u70B9\u5F00\u4E00\u8F6E" })] }), node.start === 'cron' ? (_jsx(InputRow, { label: "\u8282\u594F", value: node.rhythm ?? '', placeholder: "\u6BD4\u5982\uFF1A\u6BCF\u5929 8:00", disabled: readOnly, onChange: (value) => patch({ rhythm: value }) })) : null] })) : null, node.kind === 'end' ? _jsx("p", { className: "mt-3 text-2 text-ink-2", children: "\u8FD9\u4E00\u6761\u7EBF\u5728\u8FD9\u91CC\u7ED3\u675F\u3002" }) : null, !readOnly && node.kind !== 'start' && node.kind !== 'end' ? (_jsxs("button", { type: "button", onClick: () => props.onDeleteNode(node.id), className: "mt-5 -ml-1 rounded-btn px-1 py-1 text-2 text-fail hover:bg-fail/10", children: ["\u5220\u6389\u8FD9\u4E00\u7AD9", shared ? '（每条线都去掉）' : ''] })) : null] }));
}
export function FlowInspector(props) {
    const { flow, selectedIds, preview, readOnly, archived, instance } = props;
    let body;
    if (!selectedIds.length) {
        body = (_jsx(Overview, { flow: flow, preview: preview, readOnly: readOnly, instance: instance, onPatchFlowName: props.onPatchFlowName }));
    }
    else if (selectedIds.length > 1) {
        const selected = selectedIds
            .map((id) => flow.nodes.find((node) => node.id === id))
            .filter((node) => Boolean(node));
        body = (_jsxs("div", { children: [_jsxs("div", { className: "flex items-center gap-1", children: [_jsxs("span", { className: "text-1 text-ink-3", children: ["\u5DF2\u9009 ", selected.length, " \u4E2A"] }), _jsx("button", { type: "button", onClick: props.onClose, className: "ml-auto grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink", "aria-label": "\u53D6\u6D88\u9009\u62E9", children: _jsx(X, { className: "size-4" }) })] }), _jsx("div", { className: "mt-2 text-5 font-semibold text-ink", children: "\u6846\u9009\u7684\u7AD9" }), _jsx("ul", { className: "mt-2 space-y-1 text-2 text-ink-2", children: selected.map((node) => (_jsx("li", { children: _jsxs("button", { type: "button", onClick: () => props.onSelect(node.id), className: "flex w-full items-center gap-1.5 rounded-btn px-1 py-0.5 text-left hover:bg-hover", children: [_jsx("span", { className: "size-1.5 rounded-full bg-ink-3" }), node.name, _jsx("span", { className: "ml-auto text-1 text-ink-3", children: NODE_KIND_LABEL[node.kind] })] }) }, node.id))) }), !readOnly && !preview ? (_jsx("button", { type: "button", onClick: () => props.onDeleteNodes(selected.map((node) => node.id)), className: "mt-4 rounded-btn px-1 py-1 text-2 text-fail hover:bg-fail/10", children: "\u5220\u6389\u9009\u4E2D\u7684\u7AD9" })) : null, _jsx("p", { className: "mt-3 text-1 text-ink-3", children: "\u6846\u9009\u7528\u4E8E\u6279\u91CF\u5220\uFF1B\u70B9\u5355\u4E2A\u7AD9\u53EF\u4EE5\u6539\u8868\u5355\u3002" })] }));
    }
    else {
        const node = flow.nodes.find((item) => item.id === selectedIds[0]);
        if (!node) {
            body = (_jsx(Overview, { flow: flow, preview: preview, readOnly: readOnly, instance: instance, onPatchFlowName: props.onPatchFlowName }));
        }
        else if (preview) {
            body = _jsx(DiffDetail, { flow: flow, node: node });
        }
        else {
            body = (_jsx(Detail, { flow: flow, node: node, readOnly: readOnly, archived: archived, onPointToKind: props.onPointToKind, onClose: props.onClose, onPatchNode: props.onPatchNode, onSetNodeKind: props.onSetNodeKind, onAddToArray: props.onAddToArray, onRemoveFromArray: props.onRemoveFromArray, onAddDod: props.onAddDod, onPatchDod: props.onPatchDod, onRemoveDod: props.onRemoveDod, onDeleteNode: props.onDeleteNode }));
        }
    }
    return (_jsxs("aside", { className: "flex h-full min-h-0 w-[312px] shrink-0 flex-col border-l border-line bg-content", "aria-live": "polite", children: [_jsx("div", { className: "min-h-0 flex-1 overflow-y-auto px-4 py-3", children: body }), _jsx("div", { className: "shrink-0 border-t border-line px-4 py-2 text-1 text-ink-3", children: _jsxs("span", { className: "inline-flex items-center gap-1", children: [_jsx(ChevronDown, { className: "size-3" }), preview ? '看修改时只读' : readOnly ? (archived ? '已归档只读' : '实例态只读') : '点站改它 · 空白处取消'] }) })] }));
}
