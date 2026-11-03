import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 接不上内核时 Kind 页整页显示的状态（T007 的参照）。
 *
 * - 连不上服务：服务没起来或已经退出；页面自己隔几秒再试，也可以马上试。
 * - 令牌不对：地址里的启动令牌和这次服务的不一样（服务重启过）；用最新打印的地址重新打开。
 * - Kind 的对话不见了：状态文件指向的日志文件找不到。AIOS 不会悄悄建一条空的新对话。
 * - 日志读不懂：某一行坏了。不跳过、不重建，原样留着等你处理。
 * - 被占用：别的进程正在写这条对话（单写者）。关掉它再检查一次。
 *
 * 文案只说事实和能做的一步；路径、行号是假的样例，真的由服务报上来。
 */
import { useEffect, useState } from 'react';
import { CircleAlert, FileWarning, KeyRound, Lock, PlugZap, RotateCw } from 'lucide-react';
import { toast } from 'sonner';
import { useAppStore } from '@/app/store';
const SESSION_PATH = '~/.aios/sessions/kind-01JC8M4Q.jsonl';
function Code({ children }) {
    return _jsx("code", { className: "brand-type rounded-[4px] bg-hover px-1 py-[1px] text-2 text-ink", children: children });
}
function Block({ children }) {
    return (_jsx("pre", { className: "brand-type mt-3 overflow-x-auto rounded-btn border border-line bg-pane px-3 py-2 text-2 leading-5 text-ink-2", children: children }));
}
function useCountdown(active, every) {
    const [left, setLeft] = useState(every);
    useEffect(() => {
        if (!active)
            return;
        setLeft(every);
        const timer = window.setInterval(() => setLeft((n) => (n <= 1 ? every : n - 1)), 1000);
        return () => window.clearInterval(timer);
    }, [active, every]);
    return left;
}
export function KernelGate({ state }) {
    const setKernel = useAppStore((s) => s.setKernel);
    const left = useCountdown(state === 'unreachable', 5);
    const [checking, setChecking] = useState(false);
    // 原型里「再检查一次」总是还失败；真的由服务决定。
    const recheck = () => {
        setChecking(true);
        window.setTimeout(() => {
            setChecking(false);
            toast('还是一样', { description: '处理好之后再检查一次' });
        }, 700);
    };
    const content = {
        unreachable: {
            icon: _jsx(PlugZap, { className: "size-4" }),
            title: '连不上内核',
            body: (_jsxs(_Fragment, { children: [_jsx("p", { children: "\u5185\u6838\u7684\u670D\u52A1\u6CA1\u6709\u5728\u8FD0\u884C\uFF0C\u6216\u8005\u521A\u521A\u9000\u51FA\u4E86\u3002\u5728\u7EC8\u7AEF\u91CC\u542F\u52A8\u5B83\uFF1A" }), _jsx(Block, { children: "aios-dev serve" }), _jsxs("p", { className: "mt-3", children: ["\u542F\u52A8\u540E\u7528\u5B83\u6253\u5370\u51FA\u6765\u7684\u5730\u5740\u6253\u5F00\u3002\u8FD9\u4E00\u9875\u6BCF 5 \u79D2\u81EA\u5DF1\u518D\u8BD5\u4E00\u6B21\uFF08", _jsx("span", { className: "brand-type", children: left }), " \u79D2\u540E\uFF09\u3002"] })] })),
            action: (_jsxs("button", { type: "button", onClick: recheck, className: primary, children: [_jsx(RotateCw, { className: 'size-3.5 ' + (checking ? 'animate-spin' : '') }), "\u73B0\u5728\u91CD\u8BD5"] })),
        },
        token: {
            icon: _jsx(KeyRound, { className: "size-4" }),
            title: '这个地址已经失效',
            body: (_jsxs(_Fragment, { children: [_jsx("p", { children: "\u5185\u6838\u6BCF\u6B21\u542F\u52A8\u90FD\u4F1A\u6362\u4E00\u4E2A\u4EE4\u724C\u3002\u8FD9\u4E2A\u9875\u9762\u7528\u7684\u8FD8\u662F\u4E0A\u4E00\u6B21\u7684\uFF0C\u670D\u52A1\u62D2\u7EDD\u4E86\u5B83\u3002" }), _jsx("p", { className: "mt-2", children: "\u7528\u7EC8\u7AEF\u91CC\u6700\u65B0\u6253\u5370\u51FA\u6765\u7684\u5730\u5740\u91CD\u65B0\u6253\u5F00\u8FD9\u4E00\u9875\uFF1A" }), _jsx(Block, { children: 'AIOS 已启动：http://127.0.0.1:4417/#token=…' })] })),
        },
        lost: {
            icon: _jsx(CircleAlert, { className: "size-4" }),
            title: 'Kind 的对话找不到了',
            body: (_jsxs(_Fragment, { children: [_jsxs("p", { children: ["\u8FD9\u6761\u5BF9\u8BDD\u5E94\u8BE5\u5728 ", _jsx(Code, { children: SESSION_PATH }), "\uFF0C\u4F46\u6587\u4EF6\u4E0D\u5728\u90A3\u91CC\u3002"] }), _jsx("p", { className: "mt-2", children: "AIOS \u6CA1\u6709\u66FF\u4F60\u5EFA\u4E00\u6761\u7A7A\u7684\u65B0\u5BF9\u8BDD\uFF1A\u90A3\u6837\u4E4B\u524D\u8BF4\u8FC7\u7684\u8BDD\u5C31\u6084\u6084\u6CA1\u4E86\u3002\u5982\u679C\u662F\u4F60\u79FB\u8D70\u4E86\u5B83\uFF0C\u628A\u5B83\u653E\u56DE\u539F\u5904\u518D\u68C0\u67E5\u4E00\u6B21\u3002" })] })),
            action: (_jsxs("button", { type: "button", onClick: recheck, className: primary, children: [_jsx(RotateCw, { className: 'size-3.5 ' + (checking ? 'animate-spin' : '') }), "\u518D\u68C0\u67E5\u4E00\u6B21"] })),
        },
        broken: {
            icon: _jsx(FileWarning, { className: "size-4" }),
            title: 'Kind 的对话日志读不懂',
            body: (_jsxs(_Fragment, { children: [_jsxs("p", { children: [_jsx(Code, { children: SESSION_PATH }), " \u7684\u7B2C ", _jsx("span", { className: "brand-type", children: "1284" }), " \u884C\u4E0D\u662F\u4E00\u6761\u5B8C\u6574\u7684\u8BB0\u5F55\uFF1A"] }), _jsx(Block, { children: '{"seq":1284,"type":"message","role":"assi' }), _jsx("p", { className: "mt-3", children: "AIOS \u6CA1\u6709\u8DF3\u8FC7\u8FD9\u4E00\u884C\uFF0C\u4E5F\u6CA1\u6709\u91CD\u5EFA\u65E5\u5FD7\uFF0C\u6587\u4EF6\u539F\u6837\u7559\u7740\u3002\u5904\u7406\u597D\u4E4B\u540E\u518D\u68C0\u67E5\u4E00\u6B21\u3002" })] })),
            action: (_jsxs("button", { type: "button", onClick: recheck, className: primary, children: [_jsx(RotateCw, { className: 'size-3.5 ' + (checking ? 'animate-spin' : '') }), "\u518D\u68C0\u67E5\u4E00\u6B21"] })),
        },
        busy: {
            icon: _jsx(Lock, { className: "size-4" }),
            title: 'Kind 的对话正被别的程序占用',
            body: (_jsxs(_Fragment, { children: [_jsx("p", { children: "\u53E6\u4E00\u4E2A AIOS \u8FDB\u7A0B\u6B63\u5728\u5199\u8FD9\u6761\u5BF9\u8BDD\u3002\u540C\u4E00\u6761\u5BF9\u8BDD\u540C\u65F6\u53EA\u80FD\u6709\u4E00\u4E2A\u5199\u7684\u4EBA\uFF0C\u514D\u5F97\u4E24\u8FB9\u4E92\u76F8\u8986\u76D6\u3002" }), _jsxs("p", { className: "mt-2", children: ["\u5E38\u89C1\u7684\u662F\u7EC8\u7AEF\u91CC\u8FD8\u5F00\u7740 ", _jsx(Code, { children: "aios-dev chat" }), " \u6216\u53E6\u4E00\u4E2A ", _jsx(Code, { children: "aios-dev serve" }), "\u3002\u5173\u6389\u5B83\uFF0C\u518D\u68C0\u67E5\u4E00\u6B21\u3002"] })] })),
            action: (_jsxs("button", { type: "button", onClick: recheck, className: primary, children: [_jsx(RotateCw, { className: 'size-3.5 ' + (checking ? 'animate-spin' : '') }), "\u518D\u68C0\u67E5\u4E00\u6B21"] })),
        },
    };
    const view = content[state];
    return (_jsx("div", { className: "flex h-full items-center justify-center overflow-y-auto px-6 py-10", "data-kind": "kernel-gate", "data-state": state, children: _jsxs("div", { className: "w-full max-w-[520px]", children: [_jsxs("div", { className: "flex items-center gap-2 text-4 text-ink", children: [_jsx("span", { className: state === 'unreachable' || state === 'busy' ? 'text-wait' : 'text-fail', children: view.icon }), view.title] }), _jsx("div", { className: "mt-3 text-3 leading-6 text-ink-2", children: view.body }), _jsxs("div", { className: "mt-5 flex items-center gap-3", children: [view.action, _jsx("button", { type: "button", onClick: () => setKernel('ok'), className: "text-1 text-ink-3 hover:text-ink-2", children: "\u539F\u578B\uFF1A\u56DE\u5230\u6B63\u5E38\u72B6\u6001" })] })] }) }));
}
const primary = 'flex items-center gap-1.5 rounded-btn bg-ink px-3 py-1.5 text-2 text-content transition-opacity hover:opacity-90';
