import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 底栏的模型选择（docs 10 篇 §10.2：这里只选，不加不改；加连接、改连接去设置）。
 *
 * - 列出已保存的连接和它们的模型；连接不可用的灰掉，说原因。
 * - 选了从下一轮开始用；正在跑的这一轮不换。
 * - 最下面一行去「设置 › 模型与连接」。
 */
import { useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger, } from '@/components/ui/dropdown-menu';
import { getModelSettings } from '@/data/api';
import { go } from '@/app/navigate';
import { useKindStore } from './kindStore';
function usable(view, connectionId) {
    if (view.keyFile.problem)
        return { ok: false, why: '读不到 Keyring' };
    const conn = view.connections.find((item) => item.id === connectionId);
    if (!conn || !conn.key.saved)
        return { ok: false, why: '还没有密钥' };
    if (conn.check.kind === 'fail')
        return { ok: false, why: '连接不可用' };
    return { ok: true };
}
export function ModelPicker() {
    const model = useKindStore((state) => state.model);
    const setModel = useKindStore((state) => state.setModel);
    const [view, setView] = useState(null);
    return (_jsxs(DropdownMenu, { onOpenChange: (open) => open && void getModelSettings().then(setView), children: [_jsx(DropdownMenuTrigger, { asChild: true, children: _jsxs("button", { type: "button", className: "flex items-center gap-1 rounded-btn px-1 py-[1px] text-ink-2 transition-colors hover:bg-hover hover:text-ink", "data-kind": "model-picker", children: [model.label, _jsx(ChevronDown, { className: "size-3 opacity-70" })] }) }), _jsxs(DropdownMenuContent, { align: "start", side: "top", className: "w-64", children: [!view ? (_jsx("div", { className: "px-2 py-1.5 text-2 text-ink-3", children: "\u6B63\u5728\u8BFB\u53D6\u2026" })) : view.connections.length === 0 ? (_jsx("div", { className: "px-2 py-1.5 text-2 text-ink-3", children: "\u8FD8\u6CA1\u6709\u63A5\u4EFB\u4F55\u6A21\u578B" })) : (view.connections.map((conn) => {
                        const state = usable(view, conn.id);
                        const rows = conn.models.length
                            ? conn.models
                            : view.catalog.find((item) => item.id === conn.id)?.defaultModels ?? [];
                        return (_jsxs("div", { children: [_jsxs(DropdownMenuLabel, { className: "flex items-center gap-2 text-1 font-normal text-ink-3", children: [conn.name, state.ok ? null : _jsx("span", { className: "ml-auto text-fail", children: state.why })] }), rows.map((row) => {
                                    const on = model.connectionId === conn.id && model.modelId === row.id;
                                    return (_jsxs(DropdownMenuItem, { disabled: !state.ok, onSelect: () => !on && setModel({ connectionId: conn.id, modelId: row.id, label: row.name ?? row.id }), className: "gap-2 text-2", children: [_jsx("span", { className: "min-w-0 flex-1 truncate", children: row.name ?? row.id }), row.contextWindow ? (_jsxs("span", { className: "brand-type text-1 text-ink-3", children: [Math.round(row.contextWindow / 1000), "k"] })) : null, on ? _jsx(Check, { className: "size-3.5" }) : _jsx("span", { className: "size-3.5" })] }, row.id));
                                })] }, conn.id));
                    })), _jsx(DropdownMenuSeparator, {}), _jsx(DropdownMenuItem, { onSelect: () => go('settings:model'), className: "text-2 text-ink-2", children: "\u6DFB\u52A0\u6216\u4FEE\u6539\u8FDE\u63A5 \u203A" })] })] }));
}
