import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * Kind 右栏：上「聚焦」+ 下「Kind 现在带着」，两块同时在，不再二选一
 * （用户 2026-10-28 确认，brief §四.1「第 2 期追加」第 1 条）。
 * 原「在做的事」线路图挪到工作页的完整线路图。
 */
import { FocusPane } from './FocusPane';
import { ContextPane } from './ContextPane';
export function RightRail() {
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col overflow-hidden bg-content", "data-pane": "rail", children: [_jsx(FocusPane, {}), _jsx(ContextPane, {})] }));
}
