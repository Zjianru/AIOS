import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 模型提问卡（内核的 ask_user）：一次列出全部问题，整批作答。
 *
 * - 有选项的题：单选或多选；最后一行「其他」可以自己写（单选时写了就取代选项）。
 * - 没选项的题：直接写。
 * - 「提交回答」要每题都有答案；「不答」= 整批不答，模型收到「用户没答」后照常收尾。
 * - 答过 / 不答 / 这一轮被停了：收成几行，不再能改。
 */
import { useMemo, useState } from 'react';
import { Check, CircleHelp } from 'lucide-react';
import { useKindStore } from './kindStore';
import { cn } from '@/lib/utils';
function answered(question, draft) {
    const entry = draft[question.id];
    return Boolean(entry && (entry.selected.length > 0 || entry.custom.trim()));
}
function QuestionBlock({ question, entry, onChange, index, total }) {
    const options = question.options ?? [];
    const multi = Boolean(question.multiSelect);
    const toggle = (label) => {
        if (multi) {
            const selected = entry.selected.includes(label) ? entry.selected.filter((item) => item !== label) : entry.selected.concat(label);
            onChange({ ...entry, selected });
        }
        else {
            onChange({ selected: [label], custom: '' });
        }
    };
    return (_jsxs("fieldset", { className: "min-w-0", children: [_jsxs("legend", { className: "flex items-center gap-2 text-1 text-ink-3", children: [total > 1 ? _jsxs("span", { className: "brand-type", children: [index + 1, "/", total] }) : null, question.header ? _jsx("span", { children: question.header }) : null, multi ? _jsx("span", { children: "\u00B7 \u53EF\u4EE5\u9009\u51E0\u4E2A" }) : null] }), _jsx("div", { className: "mt-1 text-3 leading-5 text-ink", children: question.question }), options.length > 0 ? (_jsxs("div", { className: "mt-2 space-y-1", role: multi ? 'group' : 'radiogroup', children: [options.map((option) => {
                        const on = entry.selected.includes(option.label);
                        return (_jsxs("button", { type: "button", role: multi ? 'checkbox' : 'radio', "aria-checked": on, onClick: () => toggle(option.label), className: cn('flex w-full items-start gap-2.5 rounded-btn border px-2.5 py-2 text-left transition-colors', on ? 'border-ink-3/70 bg-hover' : 'border-line hover:bg-hover'), children: [_jsx("span", { className: cn('mt-[3px] grid size-3.5 shrink-0 place-items-center border', multi ? 'rounded-[3px]' : 'rounded-full', on ? 'border-ink bg-ink text-content' : 'border-line-2'), children: on ? multi ? _jsx(Check, { className: "size-2.5", strokeWidth: 3 }) : _jsx("i", { className: "size-1.5 rounded-full bg-content" }) : null }), _jsxs("span", { className: "min-w-0", children: [_jsx("span", { className: "block text-3 leading-5 text-ink", children: option.label }), option.description ? _jsx("span", { className: "block text-2 leading-5 text-ink-3", children: option.description }) : null] })] }, option.label));
                    }), _jsxs("label", { className: cn('flex items-center gap-2.5 rounded-btn border px-2.5 py-1.5 transition-colors', entry.custom.trim() ? 'border-ink-3/70 bg-hover' : 'border-line'), children: [_jsx("span", { className: "shrink-0 text-2 text-ink-3", children: "\u5176\u4ED6" }), _jsx("input", { value: entry.custom, onChange: (event) => onChange({ selected: multi || !event.target.value.trim() ? entry.selected : [], custom: event.target.value }), placeholder: "\u81EA\u5DF1\u5199", className: "min-w-0 flex-1 bg-transparent text-3 leading-6 text-ink outline-none placeholder:text-ink-3" })] })] })) : (_jsx("textarea", { rows: 2, value: entry.custom, onChange: (event) => onChange({ selected: [], custom: event.target.value }), placeholder: "\u5199\u4E0B\u4F60\u7684\u56DE\u7B54", className: "mt-2 w-full resize-none rounded-btn border border-line bg-transparent px-2.5 py-1.5 text-3 leading-5 text-ink outline-none transition-colors placeholder:text-ink-3 focus:border-ink-3/70" }))] }));
}
function PendingCard({ turnId, questions }) {
    const answer = useKindStore((state) => state.answer);
    const dismiss = useKindStore((state) => state.dismiss);
    const [draft, setDraft] = useState(() => Object.fromEntries(questions.map((question) => [question.id, { selected: [], custom: '' }])));
    const missing = useMemo(() => questions.filter((question) => !answered(question, draft)).length, [questions, draft]);
    const submit = () => {
        if (missing)
            return;
        const answers = questions.map((question) => {
            const entry = draft[question.id];
            return { id: question.id, selected: entry.selected, ...(entry.custom.trim() ? { custom: entry.custom.trim() } : {}) };
        });
        answer(turnId, answers);
    };
    return (_jsxs("div", { className: "mt-2 rounded-card border border-wait/40 bg-wait-soft/40 p-3", "data-kind": "question", onKeyDown: (event) => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                submit();
            }
        }, children: [_jsxs("div", { className: "flex items-center gap-1.5 text-2 text-wait", children: [_jsx(CircleHelp, { className: "size-3.5" }), questions.length > 1 ? `Kind 想先问你 ${questions.length} 件事` : 'Kind 想先问你一件事', _jsx("span", { className: "ml-auto text-1 text-ink-3", children: "\u5B83\u5728\u7B49\u4F60\uFF0C\u8FD9\u4E00\u8F6E\u505C\u5728\u8FD9\u91CC" })] }), _jsx("div", { className: "mt-3 space-y-4", children: questions.map((question, index) => (_jsx(QuestionBlock, { question: question, entry: draft[question.id], index: index, total: questions.length, onChange: (entry) => setDraft((current) => ({ ...current, [question.id]: entry })) }, question.id))) }), _jsxs("div", { className: "mt-4 flex items-center gap-2", children: [_jsx("span", { className: "text-1 text-ink-3", children: missing ? `还有 ${missing} 题没答` : '⌘↩ 提交' }), _jsx("button", { type: "button", onClick: () => dismiss(turnId), title: "\u6574\u6279\u4E0D\u7B54\uFF1AKind \u4F1A\u77E5\u9053\u4F60\u6CA1\u7B54\uFF0C\u7167\u5E38\u6536\u5C3E", className: "ml-auto rounded-btn px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink", children: "\u4E0D\u7B54" }), _jsx("button", { type: "button", onClick: submit, "aria-disabled": missing > 0, className: cn('rounded-btn px-3 py-1 text-2 transition-opacity', missing ? 'cursor-not-allowed bg-hover text-ink-3' : 'bg-ink text-content hover:opacity-90'), children: "\u63D0\u4EA4\u56DE\u7B54" })] })] }));
}
function answerText(question, answers) {
    const entry = answers.find((item) => item.id === question.id);
    if (!entry)
        return '—';
    return entry.selected.concat(entry.custom ? [entry.custom] : []).join('、');
}
function ClosedCard({ question }) {
    const title = question.status === 'answered'
        ? `你回答了 Kind 的 ${question.questions.length} 个问题`
        : question.status === 'dismissed'
            ? '你没回答 Kind 的问题；它知道你没答，照常收尾'
            : '这一轮停下了，问题没再等回答';
    return (_jsxs("div", { className: "mt-2 border-l border-line pl-3 text-2 leading-5", "data-kind": "question-closed", children: [_jsxs("div", { className: "flex items-center gap-1.5 text-ink-3", children: [_jsx(CircleHelp, { className: "size-3.5" }), title] }), question.status === 'answered' ? (_jsx("ul", { className: "mt-1 space-y-0.5", children: question.questions.map((item) => (_jsxs("li", { className: "text-ink-3", children: [item.question, " ", _jsx("span", { className: "text-ink-3", children: "\u2192" }), " ", _jsx("span", { className: "text-ink-2", children: answerText(item, question.answers) })] }, item.id))) })) : null] }));
}
export function QuestionCard({ turnId, question }) {
    if (question.status === 'pending')
        return _jsx(PendingCard, { turnId: turnId, questions: question.questions });
    return _jsx(ClosedCard, { question: question });
}
