import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * Kind 右栏「Kind 现在带着」（experience-spec-v1.md §6.2，brief §四.1 要求它占聚焦下面的剩余高度）。
 *
 * 只讲上下文由什么组成：每块一行 + 估算大小（≈、淡色虚线）、合计是服务商报的实际数；
 * 点开看具体每一项。角色「让他离开」、资料「拿掉」是 §6.2 允许的就地调整，带撤销。
 * 记忆「更正」要跳记忆页（第 3 期），这一版只读——写进第 2 期取舍，不在这里假做。
 */
import { useEffect, useMemo, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { toast } from 'sonner';
import { getKindContext } from '@/data/api';
import { cn } from '@/lib/utils';
function formatTokens(tokens) {
    return tokens >= 1000 ? (tokens / 1000).toFixed(1) + 'k' : String(tokens);
}
function itemKey(block, item) {
    return block.key + ':' + item.id;
}
export function ContextPane() {
    const [data, setData] = useState(null);
    const [open, setOpen] = useState(new Set());
    const [removed, setRemoved] = useState(new Set());
    useEffect(() => {
        let alive = true;
        void (async () => {
            const next = await getKindContext();
            if (alive)
                setData(next);
        })();
        return () => {
            alive = false;
        };
    }, []);
    const live = useMemo(() => {
        if (!data)
            return null;
        const blocks = data.blocks.map((block) => {
            const items = block.items.filter((item) => !removed.has(itemKey(block, item)));
            const tokens = items.reduce((sum, item) => sum + item.tokens, 0);
            return { block, items, tokens, count: block.count ?? items.length };
        });
        const total = blocks.reduce((sum, entry) => sum + entry.tokens, 0);
        return { blocks, total, changed: removed.size > 0, ratioBase: Math.max(removed.size > 0 ? total : data.reportedTotal, 1) };
    }, [data, removed]);
    if (!data || !live) {
        return (_jsx("section", { className: "flex min-h-0 flex-1 flex-col overflow-hidden", "data-block": "carrying", children: _jsx("div", { className: "px-4 py-3 text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u300CKind \u73B0\u5728\u5E26\u7740\u300D\u2026" }) }));
    }
    function toggle(key) {
        setOpen((prev) => {
            const next = new Set(prev);
            if (next.has(key))
                next.delete(key);
            else
                next.add(key);
            return next;
        });
    }
    function remove(block, item) {
        setRemoved((prev) => new Set(prev).add(itemKey(block, item)));
        toast((item.action === 'leave' ? item.label + '已离开' : item.label + ' 已拿掉'), {
            action: {
                label: '撤销',
                onClick: () => setRemoved((prev) => {
                    const next = new Set(prev);
                    next.delete(itemKey(block, item));
                    return next;
                }),
            },
        });
    }
    const totalLabel = live.changed ? '≈ ' + formatTokens(live.total) : formatTokens(data.reportedTotal);
    return (_jsxs("section", { className: "flex min-h-0 flex-1 flex-col overflow-hidden", "data-block": "carrying", children: [_jsxs("header", { className: "shrink-0 px-4 pt-3", children: [_jsxs("div", { className: "flex items-baseline gap-2", children: [_jsx("h3", { className: "text-3 font-medium text-ink", children: "Kind \u73B0\u5728\u5E26\u7740" }), _jsx("span", { className: "ml-auto border-b border-dashed border-ink-3/40 text-2 text-ink-2 brand-type", children: totalLabel })] }), _jsx("div", { className: "mt-2 flex h-2 gap-[2px] overflow-hidden rounded-badge", children: live.blocks.map((entry, index) => entry.tokens > 0 ? (_jsx("span", { title: entry.block.name + ' ≈ ' + formatTokens(entry.tokens), style: {
                                width: (entry.tokens / live.ratioBase) * 100 + '%',
                                backgroundColor: 'var(--kf-c' + (index + 1) + ')',
                            } }, entry.block.key)) : null) }), live.changed ? _jsx("p", { className: "mt-1 text-1 text-ink-3", children: "\u53D1\u51FA\u4E0B\u4E00\u6761\u6D88\u606F\u540E\u6362\u6210\u5B9E\u9645\u6570" }) : null] }), _jsx("div", { className: "min-h-0 flex-1 overflow-y-auto px-4 pt-1.5", children: live.blocks.map((entry, index) => {
                    const isOpen = open.has(entry.block.key);
                    const expandable = entry.block.items.length > 0;
                    return (_jsxs("div", { className: "border-b border-line/60 last:border-0", children: [_jsxs("button", { type: "button", disabled: !expandable, onClick: () => toggle(entry.block.key), className: cn('flex w-full items-center gap-1.5 py-1.5 text-left', expandable && 'transition-colors hover:text-ink-2'), title: expandable ? '展开看具体每一项' : undefined, children: [expandable ? (_jsx(ChevronRight, { className: cn('size-3 shrink-0 text-ink-3 transition-transform', isOpen && 'rotate-90') })) : (_jsx("span", { className: "size-3 shrink-0" })), _jsx("span", { className: "size-[9px] shrink-0 rounded-[2px]", style: { backgroundColor: 'var(--kf-c' + (index + 1) + ')' }, "aria-hidden": "true" }), _jsx("span", { className: "truncate text-2 text-ink", children: entry.block.name }), entry.block.unit ? (_jsxs("span", { className: "shrink-0 text-1 text-ink-3 brand-type", children: [entry.count, " ", entry.block.unit] })) : null, _jsx("span", { className: "ml-auto shrink-0 border-b border-dashed border-ink-3/40 text-1 text-ink-3 brand-type", children: entry.tokens === 0 ? '—' : '≈ ' + formatTokens(entry.tokens) })] }), isOpen ? (_jsxs("ul", { className: "pb-1.5 pl-5 pr-1", children: [entry.items.map((item) => (_jsxs("li", { className: "group/item flex items-baseline gap-2 py-0.5 text-1", children: [_jsxs("span", { className: "min-w-0 flex-1 truncate text-ink-3", title: item.label, children: ["\u00B7 ", item.label] }), _jsx("span", { className: "shrink-0 brand-type text-ink-3 opacity-70", children: formatTokens(item.tokens) }), item.action ? (_jsx("button", { type: "button", onClick: () => remove(entry.block, item), className: "shrink-0 text-1 text-ink-3 opacity-0 transition-opacity hover:text-ink-2 group-hover/item:opacity-100", children: item.action === 'leave' ? '让他离开' : '拿掉' })) : null] }, item.id))), entry.block.note ? (_jsx("li", { className: "pt-0.5 text-1 text-ink-3 opacity-80", children: entry.block.note })) : null] })) : null] }, entry.block.key));
                }) }), _jsx("footer", { className: "shrink-0 border-t border-line px-4 py-2 text-1 text-ink-3", children: "\u5404\u5757\u5927\u5C0F\u662F\u4F30\u7B97\uFF08\u6309\u6BCF 4 \u4E2A\u5B57\u7EA6 1 token\uFF09\uFF0C\u5408\u8BA1\u662F\u670D\u52A1\u5546\u62A5\u7684\u5B9E\u9645\u6570\u3002" })] }));
}
