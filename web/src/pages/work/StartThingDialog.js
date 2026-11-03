import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 「交给 Kind 一件事」（§四.4 三档露脸在工作页的落点）。
 *
 * 开始一件事时，Kind 要在对话里说明这次按哪档做：
 * 轻档「我直接做了」/ 普通档「先给任务图请你确认」/ 配对档「几个人并行、谁来验」。
 * 原型不真的起任务：点「开工」给一句提示。
 */
import { useState } from 'react';
import { toast } from 'sonner';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
const TIERS = [
    { id: 'light', name: '轻档', when: '小事、一句话能说清', say: '我直接做了，做完给你看结果。' },
    { id: 'normal', name: '普通档', when: '多几站、要你点一次头', say: '我先给你一张任务图，你确认了再开工。' },
    { id: 'pair', name: '配对档', when: '要求高、要有人复核', say: '两个人并行：一个人做、另一个人验，最后我核对。' },
];
export function StartThingDialog({ open, onOpenChange }) {
    const [text, setText] = useState('');
    const [tier, setTier] = useState('normal');
    const say = TIERS.find((item) => item.id === tier)?.say ?? '';
    return (_jsx(Dialog, { open: open, onOpenChange: onOpenChange, children: _jsxs(DialogContent, { className: "max-w-[560px]", children: [_jsx(DialogTitle, { className: "text-4 font-medium text-ink", children: "\u4EA4\u7ED9 Kind \u4E00\u4EF6\u4E8B" }), _jsx(DialogDescription, { className: "text-2 text-ink-3", children: "\u8BF4\u6E05\u695A\u8981\u4EC0\u4E48\u7ED3\u679C\u5C31\u884C\uFF1BKind \u6309\u9009\u4E2D\u7684\u6863\u4F4D\u5F00\u5DE5\u3002" }), _jsx("textarea", { value: text, onChange: (event) => setText(event.target.value), rows: 3, placeholder: "\u4F8B\u5982\uFF1A\u628A\u8FD9\u5468\u7684\u4F1A\u8BAE\u7EAA\u8981\u6574\u7406\u6210\u4E00\u9875", className: "mt-3 w-full resize-none rounded-btn border border-line-2 bg-pane px-2.5 py-2 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3" }), _jsx("div", { className: "mt-3 text-1 text-ink-3", children: "\u8FD9\u6B21\u6309\u54EA\u6863\u505A" }), _jsx("div", { className: "mt-1.5 grid gap-1.5", children: TIERS.map((item) => (_jsxs("button", { type: "button", onClick: () => setTier(item.id), className: cn('flex items-start gap-2 rounded-card border px-2.5 py-2 text-left transition-colors', item.id === tier ? 'border-ink-3 bg-hover' : 'border-line hover:bg-hover'), children: [_jsx("span", { className: cn('mt-1 size-3 shrink-0 rounded-full border', item.id === tier ? 'border-ink-3 bg-ink-3' : 'border-line-2') }), _jsxs("span", { className: "min-w-0", children: [_jsx("span", { className: "text-2 text-ink", children: item.name }), _jsx("span", { className: "ml-2 text-1 text-ink-3", children: item.when }), _jsx("span", { className: "mt-0.5 block text-2 leading-5 text-ink-2", children: item.say })] })] }, item.id))) }), _jsxs("div", { className: "mt-3 rounded-card bg-pane/60 p-2.5 text-2 leading-5 text-ink-2", children: [_jsx("span", { className: "text-1 text-ink-3", children: "Kind \u4F1A\u5148\u5728\u5BF9\u8BDD\u91CC\u8BF4\uFF1A" }), _jsx("br", {}), say] }), _jsxs("div", { className: "mt-3 flex justify-end gap-2", children: [_jsx("button", { type: "button", onClick: () => onOpenChange(false), className: "rounded-btn border border-line-2 px-3 py-1.5 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u53D6\u6D88" }), _jsx("button", { type: "button", disabled: !text.trim(), onClick: () => {
                                onOpenChange(false);
                                toast('（原型）已开工：' + (TIERS.find((item) => item.id === tier)?.name ?? '') + '。真起来以后，这里会换成 Kind 的对话记录。');
                            }, className: cn('rounded-btn px-3 py-1.5 text-2 transition-opacity', text.trim() ? 'bg-ink text-content hover:opacity-90' : 'cursor-not-allowed bg-hover text-ink-3'), children: "\u5F00\u5DE5" })] })] }) }));
}
