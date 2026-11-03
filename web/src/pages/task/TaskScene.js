import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 任务页（#task:<id>；§4.1 地铁线 / §4.2 节点层 / §4.4 这件事的账 / §4.5 里程碑；brief §四「工作」）。
 *
 * 左边：地铁线（点一站进下一层）、冒泡的等你事项、叶子任务的节点层、里程碑、成果；
 * 右边：这件事的账（含预计还需的依据）、问 Kind 这件事、控制这件事（暂停 / 取消）。
 * 数据只从 api 取（getTaskDetail）。
 */
import { useEffect, useState } from 'react';
import { go } from '@/app/navigate';
import { getTaskDetail } from '@/data/api';
import { toast } from 'sonner';
import { mark } from '@/lib/perf';
import { TaskLine } from '@/components/subway/TaskLine';
import { useRouteStore } from '@/app/routeStore';
import { cn } from '@/lib/utils';
const STATE_LABEL = {
    todo: '排队',
    run: '在做',
    wait: '等你',
    fail: '出了问题',
    done: '完成',
};
const STATE_PILL = {
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
function Ledger({ detail }) {
    const rows = [
        { label: '模型', value: detail.ledger.model },
        { label: '已用时间', value: detail.ledger.elapsed },
        { label: '预计还需', value: detail.ledger.eta },
        { label: 'Token', value: detail.ledger.tokens },
        { label: '费用', value: detail.ledger.cost },
    ];
    return (_jsxs("section", { className: "rounded-card border border-line bg-pane/40 p-3", children: [_jsxs("div", { className: "flex items-baseline gap-2", children: [_jsx("h3", { className: "text-3 text-ink", children: "\u8FD9\u4EF6\u4E8B\u7684\u8D26" }), _jsx("span", { className: "text-1 text-ink-3", children: "\u542B\u5168\u90E8\u5B50\u4EFB\u52A1" })] }), _jsx("dl", { className: "mt-2 space-y-1", children: rows.map((row) => (_jsxs("div", { className: "flex items-baseline justify-between gap-2 text-2", children: [_jsx("dt", { className: "text-ink-3", children: row.label }), _jsx("dd", { className: "brand-type text-ink-2", children: row.value })] }, row.label))) }), _jsx("p", { className: "mt-2 text-1 leading-4 text-ink-3", children: detail.ledger.etaNote }), _jsx("button", { type: "button", onClick: () => go('usage:' + detail.id), className: "mt-2 text-1 text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2", children: "\u7528\u91CF\u4E0E\u63D0\u9192 \u203A" })] }));
}
function ControlCard({ detail }) {
    const running = detail.children.find((child) => child.state === 'run');
    const count = detail.children.length || 1;
    return (_jsxs("section", { className: "rounded-card border border-line bg-pane/40 p-3", children: [_jsx("h3", { className: "text-3 text-ink", children: "\u63A7\u5236\u8FD9\u4EF6\u4E8B" }), _jsxs("div", { className: "mt-2 flex gap-2", children: [_jsx("button", { type: "button", onClick: () => toast('（原型）已暂停「' + detail.title + '」；随时能恢复'), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u6682\u505C\u8FD9\u4EF6\u4E8B" }), _jsx("button", { type: "button", onClick: () => toast('（原型）已取消后续工作；已经交出的成果留着'), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u53D6\u6D88" })] }), _jsxs("p", { className: "mt-2 text-1 leading-4 text-ink-3", children: ["\u6682\u505C\uFF1A\u4E0D\u518D\u5F00\u59CB\u65B0\u7684\u4E00\u6B65\u3002", running ? '正在做的「' + running.title + '」先停在一个安全的地方；' : '', "\u5DF2\u6709\u6210\u679C\u548C\u8BB0\u5F55\u90FD\u7559\u7740\uFF0C\u968F\u65F6\u80FD\u6062\u590D\u3002"] }), _jsx("p", { className: "mt-1.5 text-1 leading-4 text-ink-3", children: "\u53D6\u6D88\uFF1A\u6B63\u5728\u505A\u7684\u4F1A\u505C\u4E0B\u6536\u5C3E\uFF1B\u5DF2\u7ECF\u4EA4\u51FA\u7684\u6210\u679C\u7559\u7740\u3002\u53D6\u6D88\u540E\u4E0D\u80FD\u6062\u590D\u3002" }), _jsxs("p", { className: "mt-1.5 text-1 leading-4 text-ink-3", children: [detail.children.length ? '管这件事和它下面的 ' + count + ' 项，' : '只管这一件，', "\u4E0D\u5F71\u54CD\u522B\u7684\u4EFB\u52A1\uFF0C\u4E0D\u5220\u9664\u6210\u679C\u3002"] })] }));
}
function NodeLayer({ detail }) {
    if (!detail.nodeLayer)
        return null;
    return (_jsxs("section", { className: "mt-5", children: [_jsxs("h2", { className: "text-3 text-ink", children: ["\u600E\u4E48\u505A\u7684 \u00B7 ", detail.nodeLayer.tier] }), _jsx("div", { className: "mt-2 space-y-1.5", children: detail.nodeLayer.steps.map((step) => (_jsxs("div", { className: "flex items-center gap-2 rounded-card border border-line bg-pane/40 px-3 py-2", children: [_jsx("span", { className: "size-[7px] rounded-full bg-done" }), _jsx("span", { className: "text-2 text-ink", children: step.title }), _jsx("span", { className: "text-1 text-ink-3", children: step.who }), _jsx("span", { className: "ml-auto truncate text-1 text-ink-3", children: step.result })] }, step.title))) }), detail.nodeLayer.handover.length ? (_jsxs("div", { className: "mt-3", children: [_jsx("div", { className: "text-1 text-ink-3", children: "\u4EA4\u63A5\u8BB0\u5F55" }), _jsx("ul", { className: "mt-1 space-y-0.5", children: detail.nodeLayer.handover.map((entry) => (_jsxs("li", { className: "flex gap-2 text-1 text-ink-3", children: [_jsx("span", { className: "brand-type shrink-0", children: entry.at }), _jsx("span", { className: "shrink-0", children: entry.who }), _jsx("span", { className: "min-w-0 truncate text-ink-2", children: entry.what })] }, entry.at + entry.what))) })] })) : null, _jsx("p", { className: "mt-2 text-1 text-ink-3", children: detail.nodeLayer.machineCheck })] }));
}
function MilestoneBlock({ detail }) {
    if (!detail.milestone)
        return null;
    return (_jsxs("section", { className: "mt-5", children: [_jsxs("h2", { className: "text-3 text-ink", children: ["\uD83D\uDEA9 \u91CC\u7A0B\u7891 \u00B7 ", detail.milestone.name, _jsxs("span", { className: "ml-2 text-1 text-ink-3", children: [detail.milestone.state, detail.milestone.due ? ' · ' + detail.milestone.due : ''] })] }), _jsx("ul", { className: "mt-1.5 space-y-1", children: detail.milestone.criteria.map(([text, state, evidence]) => (_jsxs("li", { className: "flex items-baseline gap-2 text-2", children: [_jsx("span", { className: cn('shrink-0', state === 'ok' ? 'text-done' : state === 'no' ? 'text-fail' : 'text-wait'), children: state === 'ok' ? '✓' : state === 'no' ? '✗' : '○' }), _jsx("span", { className: "text-ink-2", children: text }), evidence ? _jsx("span", { className: "truncate text-1 text-ink-3", children: evidence }) : null] }, text))) })] }));
}
function Artifacts({ detail }) {
    if (!detail.artifacts.length)
        return null;
    return (_jsxs("section", { className: "mt-5", children: [_jsx("h2", { className: "text-3 text-ink", children: "\u6210\u679C" }), _jsx("div", { className: "mt-2 grid grid-cols-1 gap-2 sm:grid-cols-2", children: detail.artifacts.map((artifact, index) => (_jsxs("div", { className: "rounded-card border border-line bg-pane/40 p-2", children: [_jsx("div", { className: "grid h-[92px] place-items-center rounded-info text-1 text-white/90", style: { background: TYPE_GRADIENT[index % TYPE_GRADIENT.length] }, children: artifact.name.split('.').pop()?.toUpperCase() }), _jsx("div", { className: "mt-1.5 truncate text-2 text-ink", title: artifact.name, children: artifact.name }), _jsxs("div", { className: "mt-0.5 flex items-center justify-between text-1 text-ink-3", children: [_jsxs("span", { children: [_jsx("span", { className: "brand-type", children: artifact.version }), " \u00B7 ", artifact.size] }), _jsx("span", { className: artifact.status === 'acc' ? 'text-done' : artifact.status === 'pend' ? 'text-wait' : 'text-ink-3', children: artifact.status === 'acc' ? '✓✓ 已接受' : artifact.status === 'pend' ? '待你接受' : '草稿' })] })] }, artifact.name))) })] }));
}
export function TaskScene() {
    // 面板内容由 dockview 自己渲染；同一个场景内换路由必须订阅 routeStore，否则停在旧任务上。
    const taskId = useRouteStore((state) => state.route.id);
    const [detail, setDetail] = useState(undefined);
    useEffect(() => {
        let alive = true;
        setDetail(undefined);
        void (async () => {
            const next = await getTaskDetail(taskId);
            if (!alive)
                return;
            setDetail(next);
            // 第 2 期测速：数据到位 + 两帧 rAF 之后算「详情页打开」的终点。
            requestAnimationFrame(() => requestAnimationFrame(() => mark('task:data-painted')));
        })();
        return () => {
            alive = false;
        };
    }, [taskId]);
    if (detail === undefined) {
        return _jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u4EFB\u52A1\u2026" });
    }
    if (!detail) {
        return _jsxs("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: ["\u6CA1\u6709\u8FD9\u4E2A\u4EFB\u52A1\uFF1A", taskId] });
    }
    const attention = detail.attention;
    return (_jsx("div", { className: "h-full overflow-y-auto", "data-scene-page": "task", children: _jsxs("div", { className: "mx-auto w-full max-w-[1080px] px-6 py-5", children: [_jsxs("div", { className: "flex flex-wrap items-center gap-1 text-1 text-ink-3", children: [_jsx("span", { children: detail.projectName }), detail.path.slice(0, -1).map((entry) => (_jsxs("span", { className: "flex items-center gap-1", children: [_jsx("span", { "aria-hidden": "true", children: "\u203A" }), _jsx("span", { className: "truncate", children: entry.title })] }, entry.id)))] }), _jsxs("div", { className: "mt-1 flex flex-wrap items-center gap-2", children: [_jsx("h1", { className: "text-6 font-semibold leading-8 text-ink", children: detail.title }), _jsx("span", { className: cn('rounded-full px-2 py-[1px] text-1', STATE_PILL[detail.state]), children: STATE_LABEL[detail.state] })] }), detail.children.length ? (_jsxs("p", { className: "mt-0.5 text-2 text-ink-3", children: [detail.children.length, " \u9879\u5B50\u4EFB\u52A1 \u00B7 \u70B9\u4E00\u7AD9\u8FDB\u5165\u4E0B\u4E00\u5C42"] })) : (_jsx("p", { className: "mt-0.5 text-2 text-ink-3", children: "\u53F6\u5B50\u4EFB\u52A1 \u00B7 \u8282\u70B9\u5C42" })), _jsxs("div", { className: "mt-4 flex items-start gap-5", children: [_jsxs("div", { className: "min-w-0 flex-1", children: [detail.children.length ? _jsx(TaskLine, { detail: detail }) : null, attention ? (_jsxs("button", { type: "button", onClick: () => go('task:' + attention.id), className: "mt-3 flex w-full items-center gap-2 rounded-card border border-wait/30 bg-wait-soft/40 px-3 py-2 text-left", children: [_jsx("span", { className: "text-2 text-ink", children: attention.title }), _jsx("span", { className: "text-1 text-ink-3", children: attention.state === 'wait' ? '：等你决定' : '：出了问题' }), _jsx("span", { className: "ml-auto text-1 text-ink-3", children: "\u53BB\u770B \u203A" })] })) : null, _jsx(NodeLayer, { detail: detail }), _jsx("div", { className: "mt-2 flex justify-end", children: _jsx("button", { type: "button", onClick: () => go('trace:' + detail.id), className: "text-1 text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2", children: "\u539F\u59CB\u8BB0\u5F55\uFF08\u7B2C\u4E09\u5C42\uFF09\u203A" }) }), _jsx(MilestoneBlock, { detail: detail }), _jsx(Artifacts, { detail: detail })] }), _jsxs("aside", { className: "w-[320px] shrink-0 space-y-3", children: [_jsx(Ledger, { detail: detail }), _jsx("button", { type: "button", onClick: () => go('kind'), className: "w-full rounded-card border border-line bg-pane/40 px-3 py-2 text-left text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink", children: "\u95EE Kind \u8FD9\u4EF6\u4E8B \u203A" }), _jsx(ControlCard, { detail: detail })] })] }), _jsx("p", { className: "mt-6 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u4EFB\u52A1\u9875\uFF1A\u5730\u94C1\u7EBF\u662F\u4EFB\u52A1\u56FE\uFF08\u70B9\u4E00\u7AD9\u8FDB\u4E0B\u4E00\u5C42\uFF0C\u770B\u5230\u7684\u8FD8\u662F\u540C\u6837\u7684\u5730\u94C1\u7EBF\uFF09\uFF1B\u53F3\u8FB9\u662F\u300C\u8FD9\u4EF6\u4E8B\u7684\u8D26\u300D\u548C\u5B83\u81EA\u5DF1\u7684\u63A7\u5236\u5F00\u5173\u3002" })] }) }));
}
