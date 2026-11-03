import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 第 2 / 3 期的占位页（需求 §三.2）：写明「第 2 期」或「第 3 期」，路由与 data-go 都通。
 */
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { crumbOf } from '@/app/route';
import { PERIOD_OF } from '@/app/scenes';
import { Button } from '@/components/ui/button';
export function PeriodPage() {
    const route = useRouteStore((state) => state.route);
    const period = PERIOD_OF[route.kind] ?? 3;
    const title = crumbOf(route);
    return (_jsx("div", { className: "h-full overflow-y-auto", children: _jsxs("div", { className: "mx-auto flex min-h-full w-full max-w-[720px] flex-col justify-center px-8 py-14", children: [_jsxs("div", { className: "brand-type text-2 tracking-[0.14em] text-ink-3", children: ["\u7B2C ", period, " \u671F"] }), _jsx("h1", { className: "mt-2 text-5 font-medium text-ink", children: title }), _jsxs("p", { className: "mt-3 max-w-[560px] text-4 leading-6 text-ink-2", children: ["\u8FD9\u4E00\u9875\u6392\u5728 ", _jsxs("b", { className: "text-ink", children: ["\u7B2C ", period, " \u671F"] }), "\u3002\u7B2C 1 \u671F\u53EA\u505A\u5916\u58F3\u3001Kind \u573A\u666F\u3001\u6D41\u7A0B\u7F16\u8F91\u5668\u548C\u6D4B\u901F\uFF1B \u9700\u6C42\u89C4\u5B9A\u4E0D\u8BB8\u8DE8\u671F\u63D0\u524D\u505A\uFF0C\u6240\u4EE5\u8FD9\u91CC\u5148\u7559\u4E00\u4E2A\u5360\u4F4D\u9875\uFF0C\u8DEF\u7531\u3001\u9762\u5305\u5C51\u548C\u4FA7\u680F\u9AD8\u4EAE\u90FD\u662F\u901A\u7684\u3002"] }), _jsxs("div", { className: "mt-6 flex flex-wrap items-center gap-2", children: [_jsx(Button, { size: "sm", className: "h-8", onClick: () => go('kind'), children: "\u53BB Kind \u573A\u666F" }), _jsx(Button, { variant: "outline", size: "sm", className: "h-8", onClick: () => go('ability'), children: "\u53BB\u80FD\u529B \u2192 \u6D41\u7A0B" }), _jsx(Button, { variant: "ghost", size: "sm", className: "h-8 text-ink-2", onClick: () => go('now'), children: "\u56DE\u300C\u73B0\u5728\u300D" })] }), _jsxs("dl", { className: "mt-10 grid grid-cols-[88px_1fr] gap-y-2 border-t border-line pt-5 text-3", children: [_jsx("dt", { className: "text-ink-3", children: "\u8DEF\u7531" }), _jsxs("dd", { className: "brand-type text-ink", children: ["#", route.raw] }), _jsx("dt", { className: "text-ink-3", children: "\u573A\u666F" }), _jsx("dd", { className: "text-ink", children: route.kind }), _jsx("dt", { className: "text-ink-3", children: "\u4F9D\u636E" }), _jsx("dd", { className: "text-ink-2", children: "ROUTES.md \u7684\u671F\u6570\u8868" })] })] }) }));
}
