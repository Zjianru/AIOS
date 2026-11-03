import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 站点符号。业务图形只能手画，和 strategy-v0 的圆形站 / 叠层 / 菱形门 /
 * 托盘池 / 汇合圈 / 定时圈箭头 / 终点竖条保持同一套画法。
 *
 * 2026-10-28 地铁线规则（brief §四.2）：站是中性——一律浅底深描边，类型只靠形状；
 * 线色只留给线。填充/描边走 --stn-fill / --stn-line 两个主题 token，
 * 起点终点的实心竖条走 --ink（深色主题下是浅竖条，浅色主题下是深竖条）。
 */
export function FlowGlyph({ kind, start = 'manual', size = 24, className }) {
    const fill = 'var(--stn-fill)';
    const stroke = 'var(--stn-line)';
    const solid = 'var(--ink)';
    let inner;
    switch (kind) {
        case 'gate':
            inner = (_jsx("path", { d: "M16 3.5 L28.5 16 L16 28.5 L3.5 16 Z", fill: fill, stroke: stroke, strokeWidth: "2.4", strokeLinejoin: "round" }));
            break;
        case 'pool':
            inner = (_jsxs(_Fragment, { children: [_jsx("path", { d: "M4 10 L7 23 H25 L28 10 Z", fill: fill, stroke: stroke, strokeWidth: "2.2", strokeLinejoin: "round" }), _jsx("path", { d: "M10 16h12M11 20h10", stroke: stroke, strokeWidth: "1.5", strokeLinecap: "round" })] }));
            break;
        case 'stack':
            inner = (_jsxs(_Fragment, { children: [_jsx("rect", { x: "8", y: "4", width: "20", height: "13", rx: "3.5", fill: fill, stroke: stroke, strokeWidth: "1.5" }), _jsx("rect", { x: "5.5", y: "7", width: "20", height: "13", rx: "3.5", fill: fill, stroke: stroke, strokeWidth: "1.5" }), _jsx("rect", { x: "3", y: "10", width: "20", height: "14", rx: "3.5", fill: fill, stroke: stroke, strokeWidth: "2.4" })] }));
            break;
        case 'join':
            inner = (_jsxs(_Fragment, { children: [_jsx("circle", { cx: "16", cy: "16", r: "8.5", fill: fill, stroke: stroke, strokeWidth: "2.6" }), _jsx("circle", { cx: "16", cy: "16", r: "2.5", fill: stroke })] }));
            break;
        case 'start':
            if (start === 'cron') {
                inner = (_jsxs(_Fragment, { children: [_jsx("circle", { cx: "16", cy: "16", r: "12", fill: fill, stroke: stroke, strokeWidth: "2" }), _jsx("path", { d: "M22.7 20.6 A8 8 0 1 1 23.2 11.6", fill: "none", stroke: stroke, strokeWidth: "2.4", strokeLinecap: "round" }), _jsx("path", { d: "M23.2 7.2 L23.2 12.4 L18.2 10.4 Z", fill: stroke })] }));
            }
            else {
                inner = _jsx("rect", { x: "13.5", y: "5", width: "5", height: "22", rx: "2.5", fill: solid });
            }
            break;
        case 'end':
            inner = (_jsxs(_Fragment, { children: [_jsx("rect", { x: "13.5", y: "5", width: "5", height: "22", rx: "2.5", fill: solid }), _jsx("circle", { cx: "16", cy: "16", r: "11.5", fill: "none", stroke: "var(--ink-3)", strokeWidth: "1.2", opacity: "0.6" })] }));
            break;
        case 'station':
        default:
            inner = _jsx("circle", { cx: "16", cy: "16", r: "6.5", fill: fill, stroke: stroke, strokeWidth: "2.8" });
            break;
    }
    return (_jsx("svg", { width: size, height: size, viewBox: "0 0 32 32", className: className, "aria-hidden": "true", children: inner }));
}
