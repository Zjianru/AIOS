import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Download, FilePlus2, History, LayoutTemplate, MessageSquare, } from 'lucide-react';
import { DISTILL_OPTIONS, IMPORT_PREVIEW, NEW_FLOW_SOURCES, SAY_DEFAULT, } from '@/data/flows';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
const SOURCE_ICON = {
    blank: FilePlus2,
    say: MessageSquare,
    distil: History,
    template: LayoutTemplate,
    import: Download,
};
const primaryButton = 'h-8 rounded-btn bg-ink px-3 text-2 font-semibold text-content transition-opacity hover:opacity-90 disabled:opacity-50';
const ghostButton = 'h-8 rounded-btn border border-line-2 px-3 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink';
const optionRow = 'grid w-full grid-cols-[1fr_auto] gap-x-3 gap-y-0.5 rounded-card border border-line-2 px-3 py-2.5 text-left transition-colors hover:bg-hover';
export function NewFlowDialog({ open, onOpenChange, onCreate, templates }) {
    const [step, setStep] = useState('pick');
    const [busy, setBusy] = useState(false);
    const [peek, setPeek] = useState(false);
    const [sayText, setSayText] = useState(SAY_DEFAULT);
    const timer = useRef(null);
    useEffect(() => {
        if (open)
            return;
        setStep('pick');
        setBusy(false);
        setPeek(false);
        if (timer.current) {
            window.clearTimeout(timer.current);
            timer.current = null;
        }
    }, [open]);
    useEffect(() => () => {
        if (timer.current)
            window.clearTimeout(timer.current);
    }, []);
    const createSay = () => {
        if (busy)
            return;
        setBusy(true);
        timer.current = window.setTimeout(() => {
            timer.current = null;
            setBusy(false);
            onCreate('say');
        }, 520);
    };
    const back = (_jsxs("button", { type: "button", onClick: () => setStep('pick'), className: "inline-flex items-center gap-1 self-start text-2 text-ink-3 hover:text-ink", children: [_jsx(ArrowLeft, { className: "size-3.5" }), "\u6362\u4E00\u79CD"] }));
    let body = null;
    if (step === 'pick') {
        body = (_jsx("div", { className: "mt-4 grid grid-cols-5 gap-2.5", children: NEW_FLOW_SOURCES.map((source) => {
                const Icon = SOURCE_ICON[source.id];
                return (_jsxs("button", { type: "button", onClick: () => {
                        if (source.id === 'blank') {
                            onCreate('blank');
                            return;
                        }
                        setStep(source.id);
                    }, className: "flex flex-col items-start gap-1.5 rounded-card border border-line-2 px-3 py-3 text-left transition-colors hover:bg-hover", children: [_jsx(Icon, { className: "size-5 text-ink-2" }), _jsx("b", { className: "text-3 font-semibold text-ink", children: source.title }), _jsx("span", { className: "text-1 leading-4 text-ink-3", children: source.description })] }, source.id));
            }) }));
    }
    else if (step === 'say') {
        body = (_jsxs("div", { className: "mt-4 flex flex-col gap-3", children: [back, _jsx("textarea", { className: "min-h-[76px] w-full resize-y rounded-card border border-line-2 bg-raised px-3 py-2 text-3 leading-6 text-ink outline-none focus:border-run", value: sayText, onChange: (event) => setSayText(event.target.value), "aria-label": "\u8DDF Kind \u8BF4\u4E00\u53E5" }), _jsx("div", { className: "flex justify-end", children: _jsx("button", { type: "button", onClick: createSay, disabled: busy, className: primaryButton, children: busy ? 'Kind 在起草…' : '让 Kind 起草' }) })] }));
    }
    else if (step === 'distil') {
        body = (_jsxs("div", { className: "mt-4 flex flex-col gap-3", children: [back, _jsx("p", { className: "text-2 text-ink-3", children: "\u8FD9\u4E9B\u4E8B\u4F60\u505A\u8FC7\u4E0D\u6B62\u4E00\u6B21\u3002\u6311\u4E00\u4EF6\uFF0CKind \u628A\u6BCF\u6B21\u90FD\u4E00\u6837\u7684\u90E8\u5206\u6574\u7406\u6210\u6D41\u7A0B\uFF0C\u62FF\u4E0D\u51C6\u7684\u4F1A\u6807\u51FA\u6765\u8BA9\u4F60\u8865\u3002" }), DISTILL_OPTIONS.map((option) => (_jsxs("button", { type: "button", onClick: () => onCreate('weekly'), className: optionRow, children: [_jsx("b", { className: "text-3 font-semibold text-ink", children: option.title }), _jsx("span", { className: "text-2 text-ink-3", children: option.description }), _jsx("span", { className: "col-start-2 row-span-2 row-start-1 self-center text-2 text-ink-3", children: "\u63D0\u70BC \u2192" })] }, option.key)))] }));
    }
    else if (step === 'template') {
        body = (_jsxs("div", { className: "mt-4 flex flex-col gap-3", children: [back, templates.map((option) => (_jsxs("button", { type: "button", onClick: () => {
                        if (option.key === 'content')
                            onCreate('content');
                        else if (option.key === 'writing')
                            onCreate('writing');
                        else if (option.key === 'unfix')
                            onCreate('unfix');
                        else if (option.key === 'builtin-light')
                            onCreate('tpl-builtin-light');
                        else if (option.key === 'builtin-normal')
                            onCreate('tpl-builtin-normal');
                        else if (option.key === 'builtin-pair')
                            onCreate('tpl-builtin-pair');
                    }, className: optionRow, children: [_jsx("b", { className: "text-3 font-semibold text-ink", children: option.title }), _jsx("span", { className: "text-2 text-ink-3", children: option.description }), _jsx("span", { className: "col-start-2 row-span-2 row-start-1 self-center text-2 text-ink-3", children: "\u590D\u5236\u4E00\u4EFD \u2192" })] }, option.key)))] }));
    }
    else {
        body = (_jsxs("div", { className: "mt-4 flex flex-col gap-3", children: [back, _jsx("input", { className: "h-8 w-full rounded-btn border border-line-2 bg-raised px-2 text-2 text-ink-3 outline-none", value: IMPORT_PREVIEW.link, readOnly: true, "aria-label": "\u5206\u4EAB\u94FE\u63A5" }), peek ? (_jsxs("div", { className: "rounded-card bg-hover px-3 py-2.5", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsx("b", { className: "text-3 font-semibold text-ink", children: IMPORT_PREVIEW.title }), _jsx("span", { className: "rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2", children: IMPORT_PREVIEW.version })] }), _jsx("p", { className: "mt-1 text-2 text-ink-2", children: IMPORT_PREVIEW.summary }), _jsx("p", { className: "mt-1 text-2 text-ink-2", children: IMPORT_PREVIEW.detail })] })) : null, _jsx("div", { className: "flex justify-end gap-2", children: !peek ? (_jsx("button", { type: "button", onClick: () => setPeek(true), className: ghostButton, children: "\u770B\u770B\u91CC\u9762\u6709\u4EC0\u4E48" })) : (_jsx("button", { type: "button", onClick: () => onCreate('podcast'), className: primaryButton, children: "\u5BFC\u5165\u4E3A\u8349\u7A3F" })) })] }));
    }
    return (_jsx(Dialog, { open: open, onOpenChange: onOpenChange, children: _jsxs(DialogContent, { className: "max-w-[calc(100vw-40px)] gap-0 rounded-[14px] border-line bg-content p-5 text-ink shadow-[var(--shadow-pop)] sm:max-w-[760px]", showCloseButton: true, children: [_jsx(DialogTitle, { className: "text-4 font-semibold text-ink", children: "\u65B0\u5EFA\u6D41\u7A0B" }), _jsx(DialogDescription, { className: "mt-1 text-2 text-ink-2", children: "\u4E94\u79CD\u5F00\u5934\uFF0C\u6700\u540E\u90FD\u662F\u4E00\u4EFD\u8349\u7A3F\uFF0C\u5728\u7F16\u8F91\u5668\u91CC\u6539\u5B8C\u3001\u68C0\u67E5\u8FC7\u518D\u4FDD\u5B58\u3002" }), _jsx("div", { className: "min-h-[212px]", children: body })] }) }));
}
