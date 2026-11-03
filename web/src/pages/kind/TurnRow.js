import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 一回合：你的消息 → (思考) → 卡片 → Kind 的回答 → 回答下方一行「工作过程」。
 * 依据 experience-spec-v1.md §6.2 + prototype-to-product-v1.md §6（思考要展示，照 Codex / Claude 应用的样子）。
 *
 * 性能：整轮用 memo（比较 view 引用）；只有正在流式的那一条会重绘，别的不跟着重建。
 */
import { memo, useState } from 'react';
import { ChevronRight, CircleAlert, LoaderCircle, RotateCw, Square } from 'lucide-react';
import { Markdown } from './Markdown';
import { KindCardView } from './KindCardView';
import { QuoteChip } from './QuoteChip';
import { QuestionCard } from './QuestionCard';
import { useKindStore, useStreamStore } from './kindStore';
import { useAppStore } from '@/app/store';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
function timeOf(seconds) {
    if (!seconds)
        return '';
    const date = new Date(seconds * 1000);
    return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}
/** 纯文本 + 内联引用片（你说的话用这个，不走 markdown）。 */
function RichText({ text, elements }) {
    if (elements.length === 0)
        return _jsx(_Fragment, { children: text });
    const nodes = [];
    let cursor = 0;
    elements.forEach((element, index) => {
        if (element.start < cursor)
            return;
        if (element.start > cursor)
            nodes.push(text.slice(cursor, element.start));
        nodes.push(_jsx(QuoteChip, { element: element }, 'q' + index));
        cursor = element.end;
    });
    if (cursor < text.length)
        nodes.push(text.slice(cursor));
    return _jsx(_Fragment, { children: nodes });
}
/** 流式回答：订阅流式文本，只有这一条重绘。 */
function Answer({ view }) {
    const streamed = useStreamStore((state) => (state.turnId === view.id ? state.text : null));
    const text = streamed ?? view.answer?.text ?? '';
    const busyElsewhere = view.status !== 'inProgress' ||
        view.delivery !== null ||
        view.retry !== null ||
        view.question?.status === 'pending' ||
        view.tools.some((tool) => tool.running);
    if (!text && busyElsewhere)
        return null;
    if (!text) {
        return (_jsxs("div", { className: "flex items-center gap-2 py-1 text-3 text-ink-3", children: [_jsxs("span", { className: "inline-flex gap-1", children: [_jsx("i", { className: "size-1 animate-pulse rounded-full bg-ink-3" }), _jsx("i", { className: "size-1 animate-pulse rounded-full bg-ink-3 [animation-delay:150ms]" }), _jsx("i", { className: "size-1 animate-pulse rounded-full bg-ink-3 [animation-delay:300ms]" })] }), "\u6B63\u5728\u60F3\u2026"] }));
    }
    return _jsx(Markdown, { text: text, elements: view.answer?.elements });
}
function Thinking({ view }) {
    const [open, setOpen] = useState(false);
    const reasoning = view.reasoning;
    if (!reasoning)
        return null;
    return (_jsxs("div", { className: "mt-2", children: [_jsxs("button", { type: "button", onClick: () => setOpen((value) => !value), className: "flex items-center gap-1.5 text-2 text-ink-3 transition-colors hover:text-ink-2", children: [_jsx(ChevronRight, { className: cn('size-3.5 transition-transform', open && 'rotate-90') }), "\u5DF2\u601D\u8003 ", reasoning.seconds, " \u79D2", !open && reasoning.summary[0] ? _jsxs("span", { className: "truncate opacity-70", children: ["\u00B7 ", reasoning.summary[0]] }) : null] }), open ? (_jsxs("div", { className: "mt-1.5 border-l border-line pl-3 text-2 leading-5 text-ink-3", children: [reasoning.full ? reasoning.full.split(String.fromCharCode(10)).map((line, index) => _jsx("p", { children: line }, index)) : null, _jsx("ul", { className: "mt-1 list-disc pl-4", children: reasoning.summary.map((line) => (_jsx("li", { children: line }, line))) })] })) : null] }));
}
function WorkProcess({ view }) {
    const [open, setOpen] = useState(false);
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    const toolCount = view.tools.length;
    const seconds = view.durationMs ? Math.max(1, Math.round(view.durationMs / 1000)) : 0;
    const tokens = view.tokens.input + view.tokens.output;
    return (_jsxs("div", { className: "group/work relative mt-2 text-2", children: [_jsxs("span", { className: "pointer-events-none absolute right-0 top-0 hidden items-center gap-1.5 whitespace-nowrap text-ink-3 group-hover/work:flex", children: [view.model, " \u00B7 \u8F93\u5165 ", _jsx("span", { className: "brand-type", children: view.tokens.input }), " \u00B7 \u8F93\u51FA", ' ', _jsx("span", { className: "brand-type", children: view.tokens.output }), connectedOnly ? null : (_jsxs(_Fragment, { children: [' ', "\u00B7 \u82B1\u8D39 ", _jsxs("span", { className: "brand-type", children: ["\u00A5", view.cost?.toFixed(4) ?? '未知'] })] }))] }), _jsxs("button", { type: "button", onClick: () => setOpen((value) => !value), className: "flex items-center gap-1.5 text-ink-3 transition-colors hover:text-ink-2", children: [_jsx(ChevronRight, { className: cn('size-3.5 transition-transform', open && 'rotate-90') }), "\u5DE5\u4F5C\u8FC7\u7A0B \u00B7 ", _jsxs("span", { className: "brand-type", children: [seconds, " \u79D2"] })] }), open ? (_jsxs("div", { className: "mt-1.5 space-y-1 border-l border-line pl-3", children: [_jsxs("div", { className: "text-2 text-ink-3", children: [_jsx("span", { className: "text-ink-2", children: "\u7ECF\u8FC7" }), " \u00B7 \u60F3\u4E86 ", view.thinkingSeconds, " \u79D2 \u00B7 \u8C03\u4E86 ", toolCount, " \u6B21\u5DE5\u5177 \u00B7", ' ', _jsxs("span", { className: "brand-type", children: [(tokens / 1000).toFixed(1), "k"] })] }), view.reasoning ? (_jsxs("div", { className: "text-2 text-ink-3", children: [_jsx("span", { className: "text-ink-2", children: "\u601D\u8003" }), " \u00B7 ", view.reasoning.summary.join(' / ') || '（略）'] })) : null, view.tools.map((tool) => (_jsxs("div", { className: "flex items-start gap-2 text-2", children: [_jsx("span", { className: "mt-[1px] shrink-0 text-ink-3", children: "\u203A" }), _jsx("code", { className: "min-w-0 break-all font-mono text-ink-2", children: tool.command }), tool.output ? _jsx("span", { className: "shrink-0 text-ink-3", children: tool.output.slice(0, 40) }) : null, tool.durationMs ? (_jsxs("span", { className: "ml-auto shrink-0 brand-type text-ink-3", children: [(tool.durationMs / 1000).toFixed(1), "s"] })) : null] }, tool.id))), connectedOnly ? null : (_jsx("button", { type: "button", className: "text-2 text-ink-3 transition-colors hover:text-ink", children: "\u5728\u76D1\u63A7\u91CC\u770B \u203A" }))] })) : null] }));
}
/** 运行中的一轮：工作过程展开着，一步一步实时出现；正在跑的那步转圈。 */
function LiveWork({ view }) {
    if (view.tools.length === 0)
        return null;
    const running = view.tools.find((tool) => tool.running);
    return (_jsxs("div", { className: "mt-2 text-2", "data-kind": "live-work", children: [_jsxs("div", { className: "flex items-center gap-1.5 text-ink-3", children: [_jsx(ChevronRight, { className: "size-3.5 rotate-90" }), "\u5DE5\u4F5C\u4E2D", running ? _jsxs("span", { className: "truncate", children: ["\u00B7 \u6B63\u5728 ", _jsx("code", { className: "font-mono text-ink-2", children: running.command })] }) : null] }), _jsx("div", { className: "mt-1.5 space-y-1 border-l border-line pl-3", children: view.tools.map((tool) => (_jsxs("div", { className: "flex items-start gap-2", children: [tool.running ? (_jsx(LoaderCircle, { className: "mt-[2px] size-3 shrink-0 animate-spin text-run" })) : (_jsx("span", { className: "mt-[1px] shrink-0 text-ink-3", children: "\u203A" })), _jsx("code", { className: cn('min-w-0 break-all font-mono', tool.running ? 'text-ink' : 'text-ink-2'), children: tool.command }), tool.output ? _jsx("span", { className: "shrink-0 text-ink-3", children: tool.output }) : null, tool.durationMs ? (_jsxs("span", { className: "ml-auto shrink-0 brand-type text-ink-3", children: [(tool.durationMs / 1000).toFixed(1), "s"] })) : null] }, tool.id))) })] }));
}
/** 模型请求失败、内核在重试：说清楚第几次、为什么。 */
function RetryLine({ retry }) {
    return (_jsxs("div", { className: "mt-2 flex items-center gap-1.5 text-2 text-wait", "data-kind": "retry", children: [_jsx(RotateCw, { className: "size-3.5 animate-spin [animation-duration:2s]" }), retry.reason, "\uFF0C\u7B2C ", retry.attempt, " \u6B21\u91CD\u8BD5", _jsxs("span", { className: "text-ink-3", children: ["\u00B7 ", retry.inSeconds, " \u79D2\u540E"] })] }));
}
/** 这一轮没做完：原因原样给出；密钥类给「去设置」，其余给「再发一次」。 */
function FailureBlock({ view }) {
    const send = useKindStore((state) => state.send);
    const streaming = useKindStore((state) => state.streamingTurnId);
    const failure = view.failure;
    if (!failure)
        return null;
    const toSettings = failure.cause === 'credential' || failure.cause === 'keyring';
    return (_jsxs("div", { className: "mt-2 flex items-start gap-2 rounded-card border border-fail/40 bg-fail-soft/50 px-3 py-2 text-2", "data-kind": "failure", children: [_jsx(CircleAlert, { className: "mt-[2px] size-3.5 shrink-0 text-fail" }), _jsxs("div", { className: "min-w-0 flex-1", children: [_jsxs("div", { className: "leading-5 text-fail", children: ["\u8FD9\u4E00\u8F6E\u6CA1\u505A\u5B8C\uFF1A", failure.reason] }), _jsx("div", { className: "mt-0.5 leading-5 text-ink-3", children: failure.cause === 'keyring'
                            ? 'AIOS 不会改用别处的密钥。把钥匙文件找回来，或者在设置里重新填。'
                            : failure.cause === 'credential'
                                ? '换一把密钥，或者检查这个提供商的账户。'
                                : '内核已经按规矩重试过了。过一会儿再发一次，或者换个模型。' })] }), toSettings ? (_jsx("button", { type: "button", onClick: () => go('settings:model'), className: "shrink-0 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink", children: "\u53BB\u8BBE\u7F6E \u203A" })) : view.user ? (_jsx("button", { type: "button", disabled: Boolean(streaming), onClick: () => view.user && send(view.user.text), className: "shrink-0 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:opacity-50", children: "\u518D\u53D1\u4E00\u6B21" })) : null] }));
}
/** 压缩：对话里一条分隔线，点开看摘要。手动和自动都一样，只是标不同。 */
function CompactionDivider({ compaction }) {
    const [open, setOpen] = useState(false);
    return (_jsxs("div", { className: "px-6 py-3", "data-kind": "compaction", children: [_jsxs("div", { className: "flex items-center gap-3 text-1 text-ink-3", children: [_jsx("span", { className: "h-px flex-1 bg-line" }), _jsxs("button", { type: "button", onClick: () => setOpen((value) => !value), className: "flex items-center gap-1.5 transition-colors hover:text-ink-2", children: [_jsx(ChevronRight, { className: cn('size-3 transition-transform', open && 'rotate-90') }), "\u4E0A\u4E0B\u6587\u5DF2\u538B\u7F29 \u00B7 ", compaction.auto ? '快满了，自动' : '你手动', " \u00B7 \u5360\u7528", ' ', _jsxs("span", { className: "brand-type", children: [compaction.before, "%"] }), " \u2192 ", _jsxs("span", { className: "brand-type", children: [compaction.after, "%"] })] }), _jsx("span", { className: "h-px flex-1 bg-line" })] }), open ? (_jsxs("div", { className: "mx-auto mt-2 max-w-[560px] rounded-card border border-line bg-pane/60 px-3 py-2 text-2 leading-5 text-ink-2", children: [_jsx("div", { className: "mb-1 text-1 text-ink-3", children: "\u4E4B\u524D\u7684\u5BF9\u8BDD\u7531\u8FD9\u4EFD\u6458\u8981\u4EE3\u66FF\uFF1B\u539F\u6587\u90FD\u8FD8\u5728\u65E5\u5FD7\u91CC\u3002" }), _jsx(Markdown, { text: compaction.summary })] })) : null] }));
}
function DeliveryLine({ view }) {
    const resend = useKindStore((state) => state.resend);
    if (view.delivery === 'sending') {
        return _jsx("div", { className: "-mt-1 mb-2 text-1 text-ink-3", children: "\u53D1\u9001\u4E2D\u2026" });
    }
    if (view.delivery === 'unsent') {
        return (_jsxs("div", { className: "-mt-1 mb-2 flex items-center gap-1.5 text-1 text-fail", "data-kind": "unsent", children: [_jsx(CircleAlert, { className: "size-3" }), "\u6CA1\u9001\u5230\u5185\u6838", _jsx("button", { type: "button", onClick: () => resend(view.id), className: "rounded-btn px-1.5 py-[1px] text-ink-2 transition-colors hover:bg-hover hover:text-ink", children: "\u91CD\u53D1" }), _jsx("span", { className: "text-ink-3", children: "\u00B7 \u91CD\u53D1\u4E0D\u4F1A\u8BA9 Kind \u6536\u5230\u4E24\u6B21" })] }));
    }
    return null;
}
export const TurnRow = memo(function TurnRow({ view, dayLabel, showLastSeen, lastSeenCounts }) {
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    const streaming = view.status === 'inProgress';
    if (view.compaction)
        return _jsx(CompactionDivider, { compaction: view.compaction });
    const waitingForKernel = view.delivery !== null;
    return (_jsxs("div", { className: "px-6 pb-2", children: [dayLabel ? (_jsxs("div", { className: "mb-3 mt-4 flex items-center gap-3 text-1 text-ink-3", children: [_jsx("span", { className: "h-px flex-1 bg-line" }), _jsx("span", { className: "brand-type", children: dayLabel }), _jsx("span", { className: "h-px flex-1 bg-line" })] })) : null, showLastSeen && !connectedOnly ? (_jsxs("div", { className: "mb-3 mt-4 flex items-center gap-2 rounded-lg border border-dashed border-line-2 px-3 py-1.5 text-2 text-ink-3", children: [_jsx("span", { className: "text-ink-3", children: "\u25EF" }), "\u4E0A\u6B21\u67E5\u770B\u4E4B\u540E\uFF1A", lastSeenCounts.moved, " \u4EF6\u63A8\u8FDB\u3001", lastSeenCounts.waiting, " \u4EF6\u7B49\u4F60", _jsx(ChevronRight, { className: "ml-auto size-3.5" })] })) : null, view.user ? (_jsx("div", { className: "mb-2 mt-3 text-4 leading-6 text-ink", children: _jsx(RichText, { text: view.user.text, elements: view.user.elements }) })) : (_jsxs("div", { className: "mb-2 mt-3 flex items-center gap-1.5 text-2 text-ink-3", children: [_jsx("span", { className: "inline-block size-2 rounded-full bg-brand" }), "Kind \u00B7 ", timeOf(view.startedAt)] })), _jsx(DeliveryLine, { view: view }), view.user && !waitingForKernel ? (_jsxs("div", { className: "mb-1 flex items-center gap-1.5 text-2 text-ink-3", children: [_jsx("span", { className: "inline-block size-2 rounded-full bg-brand" }), "Kind", _jsx("span", { className: "brand-type", children: timeOf(view.startedAt) })] })) : null, waitingForKernel ? null : (_jsxs(_Fragment, { children: [_jsx(Thinking, { view: view }), connectedOnly ? null : view.cards.map((card) => _jsx(KindCardView, { card: card }, card.id)), streaming ? _jsx(LiveWork, { view: view }) : null, view.question ? _jsx(QuestionCard, { turnId: view.id, question: view.question }) : null, view.retry ? _jsx(RetryLine, { retry: view.retry }) : null, _jsx("div", { className: cn('mt-1', streaming && 'min-h-[24px]'), children: _jsx(Answer, { view: view }) }), view.status === 'interrupted' && view.stoppedAt ? (_jsxs("div", { className: "mt-1 flex items-center gap-1.5 text-2 text-ink-3", "data-kind": "stopped", children: [_jsx(Square, { className: "size-3 fill-current" }), "\u4F60\u5728 ", _jsx("span", { className: "brand-type", children: view.stoppedAt }), " \u505C\u4E0B\u4E86\u8FD9\u4E00\u8F6E\uFF0C\u5DF2\u7ECF\u505A\u7684\u7559\u7740"] })) : null, _jsx(FailureBlock, { view: view }), !streaming && (view.tools.length > 0 || view.answer) ? _jsx(WorkProcess, { view: view }) : null] }))] }));
});
