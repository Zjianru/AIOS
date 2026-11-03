import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { cn } from '@/lib/utils';
const STATE_COLOR = {
    wait: 'var(--wait)',
    fail: 'var(--fail)',
    run: 'var(--run)',
};
export function MiniLineStrip({ strip, className }) {
    const total = Math.max(strip.total, 2);
    const step = 100 / (total - 1);
    const current = Math.min(Math.max(strip.index - 1, 0), total - 1);
    const dots = Array.from({ length: total }, (_, index) => index);
    return (_jsxs("div", { className: cn('relative mt-1.5 h-2.5', className), "aria-hidden": "true", children: [_jsx("span", { className: "absolute inset-x-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full", style: { backgroundColor: strip.color, opacity: 0.28 } }), _jsx("span", { className: "absolute left-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full", style: { backgroundColor: strip.color, width: current * step + '%' } }), dots.map((index) => {
                const style = { left: index * step + '%' };
                if (index === current) {
                    return (_jsx("span", { className: "absolute top-1/2 size-[9px] -translate-x-1/2 -translate-y-1/2 rounded-full", style: {
                            ...style,
                            backgroundColor: 'var(--stn-fill)',
                            boxShadow: '0 0 0 2.5px ' + STATE_COLOR[strip.state],
                        } }, index));
                }
                return (_jsx("span", { className: "absolute top-1/2 size-[7px] -translate-x-1/2 -translate-y-1/2 rounded-full", style: { ...style, backgroundColor: strip.color, opacity: index > current ? 0.3 : 1 } }, index));
            })] }));
}
