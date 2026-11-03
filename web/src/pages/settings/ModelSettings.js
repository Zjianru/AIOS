import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 设置 › 模型与连接。照 DSH 的模型设置页（用户 2026-11-01 给的两张截图）：
 *
 * - 提供方一行一行列，一次只展开一张编辑卡片；
 * - 「添加模型提供商」分两种方式：第三方模型提供商（内置目录）、自定义模型 API；
 *   两边的草稿各自保留，切换方式不丢；
 * - 密钥框只写不读：初始为空，保存后从不回显；留空就是不动已保存的那一把；
 * - 和 DSH 不同的地方：没有环境变量后备（D34），在这里存的密钥全部进 Keyring；
 *   「已保存」和「连接可用」分开显示（docs 10 篇第 2 节）。
 *
 * 下面的「Keyring」一节：加密钥匙放在哪（用户定，D24、D41）、签发记录（docs 09 篇 7.1）。
 */
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ChevronDown, ChevronRight, CircleAlert, Eye, EyeOff, Plus, X } from 'lucide-react';
import { toast } from 'sonner';
import { useAppStore } from '@/app/store';
import { addConnection, checkConnection, discoverModels, getModelSettings, moveKeyFile, relinkKeyFile, removeConnection, KERNEL_PROVIDERS, resetKeyring, revokeKey, setKeyringProblem, syncKeyringEmpty, updateConnection, } from '@/data/api';
import { PROTOCOL_ENDPOINT_HINT, PROTOCOL_LABEL } from '@/data/fixtures/settings';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, } from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
/* ───────────────────────── 小部件 ───────────────────────── */
const inputClass = 'w-full rounded-btn border border-line-2 bg-content px-2.5 py-1.5 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3 aria-invalid:border-[var(--fail)]';
function Field({ label, help, hint, tone, children }) {
    return (_jsxs("div", { className: "mt-4 first:mt-0", children: [_jsxs("div", { className: "mb-1.5 flex items-center gap-1.5 text-2 text-ink-2", children: [label, help ? (_jsx("span", { title: help, "aria-label": help, className: "grid size-4 cursor-help place-items-center rounded-full border border-line-2 text-[10px] text-ink-3", children: "?" })) : null] }), children, hint ? (_jsx("div", { className: cn('mt-1.5 text-1 leading-4', tone ? '' : 'text-ink-3'), style: tone ? { color: `var(--${tone === 'warn' ? 'wait' : tone === 'ok' ? 'done' : 'fail'})` } : undefined, children: hint })) : null] }));
}
function Btn({ kind = 'plain', className, ...props }) {
    return (_jsx("button", { type: "button", ...props, className: cn('shrink-0 whitespace-nowrap rounded-btn px-3 py-1.5 text-2 transition-colors disabled:cursor-not-allowed disabled:opacity-45', kind === 'primary' && 'bg-ink text-content hover:opacity-90', kind === 'plain' && 'border border-line-2 text-ink-2 hover:bg-hover hover:text-ink', kind === 'quiet' && 'text-ink-3 hover:bg-hover hover:text-ink', kind === 'danger' && 'text-[var(--fail)] hover:bg-[var(--fail-soft)]', className) }));
}
/** 一行的状态点：绿 = 可用；红 = 不可用；黄 = 没有密钥；蓝 = 正在验证；灰 = 有密钥但还没验证。 */
function StatusDot({ conn, broken }) {
    const color = broken && conn.key.saved
        ? 'var(--fail)'
        : !conn.key.saved
            ? 'var(--wait)'
            : conn.check.kind === 'ok'
                ? 'var(--done)'
                : conn.check.kind === 'fail'
                    ? 'var(--fail)'
                    : conn.check.kind === 'checking'
                        ? 'var(--run)'
                        : 'var(--ink-3)';
    return (_jsx("span", { className: cn('block size-2 shrink-0 rounded-full', conn.check.kind === 'checking' && 'animate-pulse'), style: { backgroundColor: color } }));
}
/** 密钥和连接状态的一句话（「已保存」「连接可用」分开说）。 */
function keyStatus(conn, broken) {
    if (!conn || !conn.key.saved)
        return { text: '还没有密钥。填上才能用这个提供商。', tone: 'warn' };
    const saved = `已保存在 Keyring（${conn.key.savedAt}）`;
    if (broken)
        return { text: `${saved} · 现在读不出来，暂时不能用`, tone: 'fail' };
    switch (conn.check.kind) {
        case 'ok':
            return { text: `${saved} · 连接可用（${conn.check.at} 验证）`, tone: 'ok' };
        case 'fail':
            return { text: `${saved} · 不可用：${conn.check.reason}`, tone: 'fail' };
        case 'checking':
            return { text: `${saved} · 正在验证…` };
        default:
            return { text: `${saved} · 还没验证过` };
    }
}
function KeyInput({ id, value, onChange, conn }) {
    const [shown, setShown] = useState(false);
    const status = keyStatus(conn);
    return (_jsx(Field, { label: "API \u5BC6\u94A5", help: "\u5BC6\u94A5\u4EA4\u7ED9 Keyring \u52A0\u5BC6\u4FDD\u5B58\u5728\u8FD9\u53F0 Mac \u4E0A\uFF1B\u4FDD\u5B58\u4EE5\u540E\u9875\u9762\u3001\u65E5\u5FD7\u548C\u5BF9\u8BDD\u91CC\u90FD\u4E0D\u518D\u51FA\u73B0\u3002", hint: value ? undefined : status.text, tone: value ? undefined : status.tone, children: _jsxs("div", { className: "relative", children: [_jsx("input", { id: id, type: shown ? 'text' : 'password', autoComplete: "new-password", spellCheck: false, value: value, onChange: (event) => {
                        onChange(event.target.value);
                        setShown(false);
                    }, placeholder: conn?.key.saved ? '已保存一把；留空就继续用它' : '输入 API 密钥', className: cn(inputClass, value && 'pr-9') }), value ? (_jsx("button", { type: "button", onClick: () => setShown((on) => !on), "aria-label": shown ? '隐藏密钥' : '显示密钥', className: "absolute right-1.5 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink", children: shown ? _jsx(EyeOff, { className: "size-3.5" }) : _jsx(Eye, { className: "size-3.5" }) })) : null] }) }));
}
function formatTokens(n) {
    if (!n)
        return '';
    return n >= 1000000 ? n / 1000000 + 'M' : n >= 1000 ? Math.round(n / 1000) + 'K' : String(n);
}
function parseTokens(text) {
    const t = text.trim().toUpperCase();
    if (!t)
        return undefined;
    const m = /^(\d+(?:\.\d+)?)([KM]?)$/.exec(t);
    if (!m)
        return undefined;
    const base = parseFloat(m[1]);
    return Math.round(m[2] === 'M' ? base * 1000000 : m[2] === 'K' ? base * 1000 : base);
}
/* ───────────────────────── 模型目录 ───────────────────────── */
function ModelCatalog({ models, onChange, defaults, discover, required }) {
    const [candidates, setCandidates] = useState(null);
    const [picked, setPicked] = useState(new Set());
    const [query, setQuery] = useState('');
    const [asking, setAsking] = useState(false);
    const ask = async () => {
        setAsking(true);
        try {
            const list = await discover();
            setCandidates(list);
            const configured = new Set(models.map((row) => row.id));
            setPicked(new Set(models.length ? list.filter((row) => configured.has(row.id)).map((row) => row.id) : list.map((row) => row.id)));
        }
        catch (error) {
            toast.error(error.message);
        }
        finally {
            setAsking(false);
        }
    };
    const visible = (candidates ?? []).filter((row) => !query || row.id.includes(query) || (row.name ?? '').toLowerCase().includes(query.toLowerCase()));
    const applyPicked = () => {
        if (!candidates)
            return;
        const listed = new Set(candidates.map((row) => row.id));
        const kept = models.filter((row) => !listed.has(row.id) || picked.has(row.id));
        const have = new Set(kept.map((row) => row.id));
        const added = candidates.filter((row) => picked.has(row.id) && !have.has(row.id));
        onChange(kept.concat(added));
        setCandidates(null);
        setQuery('');
    };
    const setRow = (index, patch) => onChange(models.map((row, i) => (i === index ? { ...row, ...patch } : row)));
    return (_jsxs("div", { className: "mt-4 border-t border-line pt-4", children: [_jsxs("div", { className: "flex items-start", children: [_jsxs("div", { children: [_jsx("div", { className: "text-2 text-ink-2", children: "\u6A21\u578B\u76EE\u5F55" }), _jsx("div", { className: "mt-0.5 text-1 text-ink-3", children: models.length ? `${models.length} 个模型` : defaults?.length ? '正在使用适配器默认模型' : '还没有模型' })] }), _jsx(Btn, { kind: "quiet", className: "ml-auto", disabled: asking, onClick: () => void ask(), children: asking ? '正在询问…' : '获取可用模型' })] }), candidates ? (_jsxs("div", { className: "mt-2 rounded-card border border-line-2 bg-content p-2.5", "data-settings": "discover", children: [_jsx("input", { value: query, onChange: (event) => setQuery(event.target.value), placeholder: "\u641C\u7D22\u6A21\u578B", className: inputClass }), _jsxs("div", { className: "mt-2 flex items-center gap-2 text-1 text-ink-3", children: ["\u670D\u52A1\u5546\u56DE\u4E86 ", candidates.length, " \u4E2A", _jsx("button", { type: "button", className: "ml-auto hover:text-ink", onClick: () => setPicked((s) => new Set([...s, ...visible.map((r) => r.id)])), children: "\u5168\u9009" }), _jsx("button", { type: "button", className: "hover:text-ink", onClick: () => setPicked((s) => new Set([...s].filter((id) => !visible.some((r) => r.id === id)))), children: "\u53D6\u6D88\u5168\u9009" })] }), _jsx("div", { className: "mt-1.5 max-h-[180px] overflow-y-auto", children: visible.map((row) => (_jsxs("label", { className: "flex cursor-pointer items-center gap-2 rounded-btn px-1.5 py-1 hover:bg-hover", title: row.name ?? row.id, children: [_jsx("input", { type: "checkbox", checked: picked.has(row.id), onChange: (event) => setPicked((s) => {
                                        const next = new Set(s);
                                        if (event.target.checked)
                                            next.add(row.id);
                                        else
                                            next.delete(row.id);
                                        return next;
                                    }) }), _jsx("span", { className: "brand-type truncate text-2 text-ink", children: row.id }), row.contextWindow ? _jsx("span", { className: "ml-auto text-1 text-ink-3", children: formatTokens(row.contextWindow) }) : null] }, row.id))) }), _jsxs("div", { className: "mt-2 flex justify-end gap-2", children: [_jsx(Btn, { kind: "quiet", onClick: () => setCandidates(null), children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", disabled: models.length > 0 && picked.size === 0, onClick: applyPicked, children: "\u66F4\u65B0\u5217\u8868" })] })] })) : null, models.length ? (_jsxs("div", { className: "mt-2 space-y-1.5", children: [_jsxs("div", { className: "grid grid-cols-[1fr_1fr_64px_64px_24px] gap-1.5 px-0.5 text-1 text-ink-3", children: [_jsx("span", { children: "\u6A21\u578B ID" }), _jsx("span", { children: "\u663E\u793A\u540D\u79F0" }), _jsx("span", { children: "\u4E0A\u4E0B\u6587" }), _jsx("span", { children: "\u6700\u5927\u8F93\u51FA" }), _jsx("span", {})] }), models.map((row, index) => (_jsxs("div", { className: "grid grid-cols-[1fr_1fr_64px_64px_24px] items-center gap-1.5", children: [_jsx("input", { className: cn(inputClass, 'brand-type'), value: row.id, placeholder: "model-id", onChange: (e) => setRow(index, { id: e.target.value }) }), _jsx("input", { className: inputClass, value: row.name ?? '', placeholder: "\u53EF\u4E0D\u586B", onChange: (e) => setRow(index, { name: e.target.value || undefined }) }), _jsx("input", { className: inputClass, defaultValue: formatTokens(row.contextWindow), placeholder: "\u7EE7\u627F", onBlur: (e) => setRow(index, { contextWindow: parseTokens(e.target.value) }) }, 'cw' + index + ':' + row.contextWindow), _jsx("input", { className: inputClass, defaultValue: formatTokens(row.maxTokens), placeholder: "\u7EE7\u627F", onBlur: (e) => setRow(index, { maxTokens: parseTokens(e.target.value) }) }, 'mt' + index + ':' + row.maxTokens), _jsx("button", { type: "button", "aria-label": "\u5220\u6389\u8FD9\u4E2A\u6A21\u578B", onClick: () => onChange(models.filter((_, i) => i !== index)), className: "grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink", children: _jsx(X, { className: "size-3.5" }) })] }, index)))] })) : (_jsx("div", { className: "mt-2 rounded-card border border-dashed border-line-2 px-3 py-3 text-center text-2 leading-5 text-ink-3", style: required ? { color: 'var(--wait)' } : undefined, children: defaults?.length
                    ? `模型选择器里列出默认的 ${defaults.map((row) => row.id).join('、')}；要改就在这里加。`
                    : required
                        ? '自定义接口至少要写一个模型。'
                        : '模型选择器里不会列出模型；目录外的 ID 仍可直接发送。' })), _jsxs(Btn, { kind: "plain", className: "mt-2.5 inline-flex items-center gap-1", onClick: () => onChange(models.concat({ id: '' })), children: [_jsx(Plus, { className: "size-3.5" }), "\u6DFB\u52A0\u6A21\u578B"] })] }));
}
function Collapsible({ title, children, defaultOpen = false }) {
    const [open, setOpen] = useState(defaultOpen);
    return (_jsxs("div", { className: "mt-4 border-t border-line pt-3", children: [_jsxs("button", { type: "button", onClick: () => setOpen((on) => !on), className: "flex items-center gap-1 text-2 text-ink-2 hover:text-ink", "aria-expanded": open, children: [open ? _jsx(ChevronDown, { className: "size-3.5" }) : _jsx(ChevronRight, { className: "size-3.5" }), title] }), open ? _jsx("div", { className: "mt-3", children: children }) : null] }));
}
/* ───────────────────────── 已有提供方的编辑卡片 ───────────────────────── */
function ConnectionEditor({ conn, view, onDone, onView }) {
    const [key, setKey] = useState('');
    const [endpoint, setEndpoint] = useState(conn.endpoint);
    const [models, setModels] = useState(conn.models);
    const [saving, setSaving] = useState(false);
    const [confirm, setConfirm] = useState(null);
    const catalog = view.catalog.find((item) => item.id === conn.id);
    const save = async () => {
        setSaving(true);
        try {
            const withKey = Boolean(key.trim());
            onView(await updateConnection(conn.id, { endpoint, models, key }));
            onDone();
            toast.success(withKey ? `已保存；${conn.name} 的新密钥进了 Keyring，正在验证` : '已保存');
            if (withKey)
                onView(await checkConnection(conn.id));
        }
        catch (error) {
            toast.error(error.message);
        }
        finally {
            setSaving(false);
        }
    };
    const act = async () => {
        const what = confirm;
        setConfirm(null);
        if (what === 'revoke') {
            onView(await revokeKey(conn.id));
            toast(`${conn.name} 的密钥已撤销，之后的请求不再签发`);
        }
        else if (what === 'remove') {
            onView(await removeConnection(conn.id));
            onDone();
            toast(`已删除 ${conn.name}，它的密钥一起撤销了`);
        }
    };
    return (_jsxs("div", { className: "mx-1 mb-2 rounded-card border border-line-2 bg-raised/60 p-4", "data-settings": "editor", children: [_jsx(KeyInput, { id: 'key-' + conn.id, value: key, onChange: setKey, conn: conn }), _jsxs(Collapsible, { title: "\u81EA\u5B9A\u4E49\u8BBE\u7F6E", defaultOpen: conn.source === 'custom', children: [_jsx(Field, { label: "API \u5730\u5740", hint: `协议：${PROTOCOL_LABEL[conn.protocol]}`, children: _jsx("input", { className: inputClass, value: endpoint, onChange: (e) => setEndpoint(e.target.value), placeholder: conn.defaultEndpoint }) }), _jsx(ModelCatalog, { models: models, onChange: setModels, defaults: catalog?.defaultModels, required: conn.source === 'custom', discover: () => discoverModels({ catalogId: catalog?.id, endpoint: endpoint || conn.defaultEndpoint }) })] }), _jsxs("div", { className: "mt-5 flex items-center gap-2", children: [conn.key.saved ? _jsx(Btn, { kind: "danger", onClick: () => setConfirm('revoke'), children: "\u64A4\u9500\u5BC6\u94A5" }) : null, _jsx(Btn, { kind: "quiet", onClick: () => setConfirm('remove'), children: "\u5220\u9664\u8FD9\u4E2A\u63D0\u4F9B\u5546" }), _jsx(Btn, { kind: "plain", className: "ml-auto", onClick: onDone, children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", disabled: saving, onClick: () => void save(), children: saving ? '正在保存…' : '保存' })] }), _jsx(Dialog, { open: confirm !== null, onOpenChange: (open) => (open ? null : setConfirm(null)), children: _jsxs(DialogContent, { children: [_jsxs(DialogHeader, { children: [_jsx(DialogTitle, { children: confirm === 'revoke' ? `撤销 ${conn.name} 的密钥？` : `删除 ${conn.name}？` }), _jsx(DialogDescription, { children: confirm === 'revoke'
                                        ? '立刻生效：之后用它的模型请求都会停下，Kind 正在做的事会停在「等连接」，直到你再填一把。撤销会留下记录，密钥本身不留。'
                                        : '这个提供商的地址、模型目录一起删掉；它在 Keyring 里的密钥同时撤销。用它的模型请求会停下。' })] }), _jsxs(DialogFooter, { children: [_jsx(Btn, { kind: "plain", onClick: () => setConfirm(null), children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", className: "!bg-[var(--fail)] !text-white", onClick: () => void act(), children: confirm === 'revoke' ? '撤销' : '删除' })] })] }) })] }));
}
function CatalogPanel({ view, onCreated, onCancel }) {
    const choices = view.catalog.filter((item) => !view.connections.some((conn) => conn.id === item.id));
    const [providerId, setProviderId] = useState(choices[0]?.id ?? '');
    const [key, setKey] = useState('');
    const [endpoint, setEndpoint] = useState('');
    const [models, setModels] = useState([]);
    const [saving, setSaving] = useState(false);
    const provider = choices.find((item) => item.id === providerId) ?? choices[0];
    if (!provider) {
        return _jsx("p", { className: "text-2 text-ink-3", children: "\u5185\u7F6E\u76EE\u5F55\u91CC\u7684\u63D0\u4F9B\u5546\u90FD\u5DF2\u7ECF\u52A0\u8FC7\u4E86\u3002" });
    }
    const save = async () => {
        setSaving(true);
        try {
            const next = await addConnection({
                source: 'catalog', id: provider.id, name: provider.name, protocol: provider.protocol, endpoint, models, key,
            });
            onCreated(provider.id, Boolean(key.trim()), next);
        }
        catch (error) {
            toast.error(error.message);
        }
        finally {
            setSaving(false);
        }
    };
    return (_jsxs("div", { children: [_jsx(Field, { label: "\u63D0\u4F9B\u5546", children: _jsx("select", { value: provider.id, onChange: (e) => {
                        setProviderId(e.target.value);
                        setModels([]);
                        setEndpoint('');
                    }, className: cn(inputClass, 'w-auto min-w-[240px]'), children: choices.map((item) => (_jsx("option", { value: item.id, children: item.name }, item.id))) }) }), _jsx(KeyInput, { id: "add-catalog-key", value: key, onChange: setKey, conn: null }), _jsxs(Collapsible, { title: "\u81EA\u5B9A\u4E49\u8BBE\u7F6E", children: [_jsx(Field, { label: "API \u5730\u5740", hint: `协议：${PROTOCOL_LABEL[provider.protocol]}`, children: _jsx("input", { className: inputClass, value: endpoint, onChange: (e) => setEndpoint(e.target.value), placeholder: provider.defaultEndpoint }) }), _jsx(ModelCatalog, { models: models, onChange: setModels, defaults: provider.defaultModels, discover: () => discoverModels({ catalogId: provider.id, endpoint: endpoint || provider.defaultEndpoint }) })] }), _jsxs("div", { className: "mt-5 flex justify-end gap-2", children: [_jsx(Btn, { kind: "plain", onClick: onCancel, children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", disabled: saving, onClick: () => void save(), children: saving ? '正在保存…' : '保存' })] })] }));
}
function CustomPanel({ onCreated, onCancel }) {
    const [id, setId] = useState('');
    const [name, setName] = useState('');
    const [endpoint, setEndpoint] = useState('');
    const [protocol, setProtocol] = useState('openai-chat');
    const [key, setKey] = useState('');
    const [models, setModels] = useState([]);
    const [saving, setSaving] = useState(false);
    const idBad = id !== '' && !/^[a-z][a-z0-9-]*$/.test(id);
    const endpointBad = endpoint !== '' && !/^https?:\/\/[^\s/]+/.test(endpoint);
    const ready = id && !idBad && endpoint && !endpointBad && models.some((row) => row.id.trim());
    const save = async () => {
        setSaving(true);
        try {
            const next = await addConnection({ source: 'custom', id, name, protocol, endpoint, models, key });
            onCreated(id, Boolean(key.trim()), next);
        }
        catch (error) {
            toast.error(error.message);
        }
        finally {
            setSaving(false);
        }
    };
    return (_jsxs("div", { children: [_jsx(Field, { label: "Provider ID", hint: idBad ? '要以小写字母开头，只用小写字母、数字和 -' : '以小写字母开头的标识，在请求里唯一标识这个提供商，Keyring 里的凭证也按它登记。', tone: idBad ? 'fail' : undefined, children: _jsx("input", { className: cn(inputClass, 'brand-type'), "aria-invalid": idBad, value: id, onChange: (e) => setId(e.target.value), placeholder: "acme-gateway" }) }), _jsx(Field, { label: "\u663E\u793A\u540D\u79F0", children: _jsx("input", { className: inputClass, value: name, onChange: (e) => setName(e.target.value), placeholder: "\u663E\u793A\u540D\u79F0" }) }), _jsx(Field, { label: "API \u5730\u5740", hint: endpointBad ? '要是 http 或 https 开头的完整地址' : undefined, tone: endpointBad ? 'fail' : undefined, children: _jsx("input", { className: inputClass, "aria-invalid": endpointBad, value: endpoint, onChange: (e) => setEndpoint(e.target.value), placeholder: PROTOCOL_ENDPOINT_HINT[protocol] }) }), _jsx(Field, { label: "API \u534F\u8BAE", children: _jsx("select", { value: protocol, onChange: (e) => setProtocol(e.target.value), className: cn(inputClass, 'w-auto min-w-[240px]'), children: Object.keys(PROTOCOL_LABEL).map((item) => (_jsx("option", { value: item, children: PROTOCOL_LABEL[item] }, item))) }) }), _jsx(KeyInput, { id: "add-custom-key", value: key, onChange: setKey, conn: null }), _jsx(ModelCatalog, { models: models, onChange: setModels, required: true, discover: () => discoverModels({ endpoint }) }), _jsxs("div", { className: "mt-5 flex justify-end gap-2", children: [_jsx(Btn, { kind: "plain", onClick: onCancel, children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", disabled: !ready || saving, onClick: () => void save(), children: saving ? '正在创建…' : '创建提供商' })] })] }));
}
function AddCard({ view, onCreated, onCancel }) {
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    const [mode, setMode] = useState('catalog');
    const [mounted, setMounted] = useState(new Set(['catalog']));
    const pick = (next) => {
        setMode(next);
        setMounted((s) => new Set([...s, next]));
    };
    const lead = mode === 'catalog'
        ? '从内置目录里选 DeepSeek、Kimi、OpenAI 等提供商，填上 API 密钥就能用。'
        : '连接中转站、自己部署的服务，或者其他兼容 OpenAI / Anthropic 协议的接口；要写 API 地址、协议和模型。';
    if (connectedOnly) {
        const kernelView = { ...view, catalog: view.catalog.filter((item) => KERNEL_PROVIDERS.has(item.id)) };
        return (_jsxs("div", { className: "mt-2 rounded-card border border-line-2 bg-raised/60 p-4", "data-settings": "add", children: [_jsx("p", { className: "mb-4 text-1 leading-4 text-ink-3", children: "Kind \u73B0\u5728\u53EA\u80FD\u7528 DeepSeek\uFF0C\u586B\u4E0A API \u5BC6\u94A5\u5C31\u80FD\u7528\u3002\u5176\u4ED6\u63D0\u4F9B\u5546\u4E4B\u540E\u518D\u52A0\u3002" }), _jsx(CatalogPanel, { view: kernelView, onCreated: onCreated, onCancel: onCancel })] }));
    }
    return (_jsxs("div", { className: "mt-2 rounded-card border border-line-2 bg-raised/60 p-4", "data-settings": "add", children: [_jsx("div", { className: "inline-flex rounded-btn border border-line-2 bg-content p-0.5", role: "tablist", children: ['catalog', 'custom'].map((item) => (_jsx("button", { type: "button", role: "tab", "aria-selected": mode === item, onClick: () => pick(item), className: cn('rounded-[6px] px-3 py-1 text-2 transition-colors', mode === item ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink'), children: item === 'catalog' ? '第三方模型提供商' : '自定义模型 API' }, item))) }), _jsx("p", { className: "mb-4 mt-2 text-1 leading-4 text-ink-3", children: lead }), mounted.has('catalog') ? (_jsx("div", { hidden: mode !== 'catalog', children: _jsx(CatalogPanel, { view: view, onCreated: onCreated, onCancel: onCancel }) })) : null, mounted.has('custom') ? (_jsx("div", { hidden: mode !== 'custom', children: _jsx(CustomPanel, { onCreated: onCreated, onCancel: onCancel }) })) : null] }));
}
/* ───────────────────────── Keyring 读不出来 ───────────────────────── */
/**
 * 钥匙文件不在原处、打不开或解不开（09 篇第 7 节）：
 * - 所有提供商一起停用，不悄悄改用环境变量或别处的密钥；
 * - 能做的两步：告诉 AIOS 钥匙文件现在在哪（只改指向）；找不回就重新开始（换新钥匙，密钥重填）。
 */
function KeyringBroken({ view, onView }) {
    const [relinking, setRelinking] = useState(false);
    const [path, setPath] = useState('');
    const [busy, setBusy] = useState(false);
    const [confirmReset, setConfirmReset] = useState(false);
    const setKeyringBroken = useAppStore((state) => state.setKeyringBroken);
    const relink = async () => {
        setBusy(true);
        try {
            const next = await relinkKeyFile(path);
            setKeyringProblem(null);
            setKeyringBroken(false);
            onView(next);
            toast.success('找到了，Keyring 又能读出来了');
        }
        catch (error) {
            toast.error(error.message);
        }
        finally {
            setBusy(false);
        }
    };
    const reset = async () => {
        const next = await resetKeyring();
        setKeyringProblem(null);
        setKeyringBroken(false);
        setConfirmReset(false);
        onView(next);
        toast.success('换了一把新钥匙；给每个提供商重新填一次密钥');
    };
    return (_jsxs("div", { className: "mt-5 rounded-card border border-fail/40 bg-fail-soft/40 px-4 py-3", "data-settings": "keyring-broken", children: [_jsxs("div", { className: "flex items-start gap-2", children: [_jsx(CircleAlert, { className: "mt-[3px] size-3.5 shrink-0 text-fail" }), _jsxs("div", { className: "min-w-0 text-2 leading-5", children: [_jsx("div", { className: "text-3 text-ink", children: "Keyring \u73B0\u5728\u8BFB\u4E0D\u51FA\u6765" }), _jsxs("p", { className: "mt-0.5 text-ink-2", children: ["\u52A0\u5BC6\u94A5\u5319 ", _jsx("span", { className: "brand-type", children: view.keyFile.path }), "\uFF1A", view.keyFile.problem, "\u3002 \u6240\u6709\u63D0\u4F9B\u5546\u90FD\u6682\u65F6\u4E0D\u80FD\u7528\uFF1BAIOS \u4E0D\u4F1A\u6539\u7528\u73AF\u5883\u53D8\u91CF\u6216\u522B\u5904\u7684\u5BC6\u94A5\u3002"] })] })] }), relinking ? (_jsxs("div", { className: "mt-3 flex items-center gap-2 pl-5.5", children: [_jsx("input", { autoFocus: true, className: cn(inputClass, 'brand-type'), value: path, onChange: (event) => setPath(event.target.value), placeholder: "\u94A5\u5319\u6587\u4EF6\u73B0\u5728\u7684\u5B8C\u6574\u8DEF\u5F84\uFF0C\u6BD4\u5982 ~/Backups/keyring.key", onKeyDown: (event) => event.key === 'Enter' && void relink() }), _jsx(Btn, { kind: "plain", onClick: () => setRelinking(false), children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", disabled: busy || !path.trim(), onClick: () => void relink(), children: busy ? '正在读…' : '用这个' })] })) : (_jsxs("div", { className: "mt-3 flex items-center gap-2 pl-5.5", children: [_jsx(Btn, { kind: "primary", onClick: () => setRelinking(true), children: "\u6307\u5230\u5B83\u73B0\u5728\u7684\u4F4D\u7F6E" }), _jsx(Btn, { kind: "danger", onClick: () => setConfirmReset(true), children: "\u627E\u4E0D\u56DE\u4E86\uFF0C\u91CD\u65B0\u5F00\u59CB" })] })), _jsx(Dialog, { open: confirmReset, onOpenChange: (open) => (open ? null : setConfirmReset(false)), children: _jsxs(DialogContent, { children: [_jsxs(DialogHeader, { children: [_jsx(DialogTitle, { children: "\u6362\u4E00\u628A\u65B0\u94A5\u5319\uFF0C\u91CD\u65B0\u5F00\u59CB\uFF1F" }), _jsx(DialogDescription, { children: "\u65E7\u94A5\u5319\u52A0\u5BC6\u7684\u5BC6\u94A5\u90FD\u8BFB\u4E0D\u56DE\u6765\u4E86\uFF1A\u6BCF\u4E2A\u63D0\u4F9B\u5546\u8981\u91CD\u65B0\u586B\u4E00\u6B21\u5BC6\u94A5\u3002\u63D0\u4F9B\u5546\u7684\u5730\u5740\u548C\u6A21\u578B\u76EE\u5F55\u90FD\u7559\u7740\u3002\u65E7\u94A5\u5319\u4EE5\u540E\u627E\u56DE\u6765\u4E5F\u7528\u4E0D\u4E0A\u4E86\u3002" })] }), _jsxs(DialogFooter, { children: [_jsx(Btn, { kind: "plain", onClick: () => setConfirmReset(false), children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", className: "!bg-[var(--fail)] !text-white", onClick: () => void reset(), children: "\u91CD\u65B0\u5F00\u59CB" })] })] }) })] }));
}
/* ───────────────────────── Keyring 一节 ───────────────────────── */
function KeyringSection({ view, onView }) {
    const [moving, setMoving] = useState(false);
    const [path, setPath] = useState(view.keyFile.path);
    const [showIssues, setShowIssues] = useState(false);
    const move = async () => {
        try {
            onView(await moveKeyFile(path));
            setMoving(false);
            toast.success('钥匙已经挪过去，旧位置的文件删掉了');
        }
        catch (error) {
            toast.error(error.message);
        }
    };
    return (_jsxs("section", { className: "mt-10", "data-settings": "keyring", children: [_jsx("h2", { className: "text-3 font-medium text-ink", children: "Keyring" }), _jsx("p", { className: "mt-1 text-2 leading-5 text-ink-2", children: "\u4E0A\u9762\u5B58\u7684\u5BC6\u94A5\u90FD\u5728\u8FD9\u91CC\uFF0C\u52A0\u5BC6\u4FDD\u5B58\u3002Kind \u548C\u6A21\u578B\u4ECE\u4E0D\u62FF\u5230\u5BC6\u94A5\u672C\u8EAB\uFF1A\u6BCF\u6B21\u8BF7\u6C42\u7531\u5185\u6838\u5411 Keyring \u7533\u8BF7\u4E00\u6B21\uFF0C\u53EA\u5728\u53D1\u51FA\u7684\u90A3\u4E00\u523B\u5C55\u5F00\u3002" }), _jsxs("div", { className: "mt-3 divide-y divide-line rounded-card border border-line", children: [_jsxs("div", { className: "px-4 py-3", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsxs("div", { className: "min-w-0", children: [_jsx("div", { className: "text-3 text-ink", children: "\u52A0\u5BC6\u94A5\u5319" }), _jsxs("div", { className: "mt-0.5 text-1 text-ink-3", children: ["\u653E\u5728\u54EA\u7531\u4F60\u5B9A\u3002\u73B0\u5728\u662F\u4E00\u4E2A\u6587\u4EF6\uFF0C", view.keyFile.since, "\u8D77\u653E\u5728\u8FD9\u91CC\u3002"] })] }), moving || view.keyFile.problem ? null : _jsx(Btn, { kind: "plain", className: "ml-auto", onClick: () => setMoving(true), children: "\u6362\u4F4D\u7F6E" })] }), moving ? (_jsxs("div", { className: "mt-2.5 flex items-center gap-2", children: [_jsx("input", { className: cn(inputClass, 'brand-type'), value: path, onChange: (e) => setPath(e.target.value) }), _jsx(Btn, { kind: "plain", onClick: () => { setMoving(false); setPath(view.keyFile.path); }, children: "\u53D6\u6D88" }), _jsx(Btn, { kind: "primary", onClick: () => void move(), children: "\u632A\u8FC7\u53BB" })] })) : (_jsxs("div", { className: "mt-2 flex items-center gap-2 text-2", children: [_jsx("span", { className: cn('brand-type truncate', view.keyFile.problem ? 'text-ink-3 line-through' : 'text-ink-2'), children: view.keyFile.path }), view.keyFile.problem ? _jsx("span", { className: "shrink-0 text-fail", children: view.keyFile.problem }) : null] })), _jsx("p", { className: "mt-2.5 text-1 leading-4 text-ink-3", children: "\u8FD9\u53F0 Mac \u4E0A\u4EE5\u4F60\u7684\u8EAB\u4EFD\u8FD0\u884C\u7684\u7A0B\u5E8F\uFF0C\u5305\u62EC Kind \u66FF\u4F60\u6267\u884C\u7684\u547D\u4EE4\uFF0C\u90FD\u80FD\u8BFB\u5230\u8FD9\u4E2A\u6587\u4EF6\u3002\u52A0\u5BC6\u9632\u7684\u662F\u6570\u636E\u88AB\u590D\u5236\u3001\u540C\u6B65\u6216\u5907\u4EFD\u51FA\u53BB\uFF0C\u6321\u4E0D\u4F4F\u672C\u673A\u4E0A\u7684\u7A0B\u5E8F\u3002" })] }), _jsxs("div", { className: "px-4 py-3", children: [_jsxs("div", { className: "flex items-center gap-2", children: [_jsxs("div", { children: [_jsx("div", { className: "text-3 text-ink", children: "\u7B7E\u53D1\u8BB0\u5F55" }), _jsx("div", { className: "mt-0.5 text-1 text-ink-3", children: view.issuedToday ? `今天签发 ${view.issuedToday} 次，最近一次 ${view.issues[0]?.at ?? '—'}。只记谁用、做什么，不记密钥。` : '今天还没有签发过。' })] }), view.issues.length ? (_jsx(Btn, { kind: "quiet", className: "ml-auto", onClick: () => setShowIssues((on) => !on), children: showIssues ? '收起' : '看最近的' })) : null] }), showIssues ? (_jsxs("table", { className: "mt-2.5 w-full text-2", children: [_jsx("thead", { children: _jsxs("tr", { className: "text-left text-1 text-ink-3", children: [_jsx("th", { className: "py-1 font-normal", children: "\u65F6\u95F4" }), _jsx("th", { className: "py-1 font-normal", children: "\u63D0\u4F9B\u5546" }), _jsx("th", { className: "py-1 font-normal", children: "\u7528\u9014" }), _jsx("th", { className: "py-1 font-normal", children: "\u6709\u6548\u5230" })] }) }), _jsx("tbody", { children: view.issues.map((row) => (_jsxs("tr", { className: "border-t border-line text-ink-2", children: [_jsx("td", { className: "brand-type py-1", children: row.at }), _jsx("td", { className: "py-1", children: row.connection }), _jsx("td", { className: "py-1", children: row.purpose }), _jsx("td", { className: "brand-type py-1", children: row.until })] }, row.id))) })] })) : null] })] })] }));
}
/* ───────────────────────── 整组 ───────────────────────── */
export function ModelSettings() {
    const firstUse = useAppStore((state) => state.firstUse);
    const keyringBroken = useAppStore((state) => state.keyringBroken);
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    const [view, setView] = useState(null);
    /** 展开的是哪张卡片：连接 id、'add'，或者都不展开。一次只展开一张。 */
    const [open, setOpen] = useState(null);
    const load = useCallback(async () => {
        syncKeyringEmpty(firstUse);
        const next = await getModelSettings();
        setView(next);
        // 首次运行：一个提供商都没有时，直接展开添加卡片
        if (!next.connections.length)
            setOpen('add');
    }, [firstUse, keyringBroken]);
    useEffect(() => {
        void load();
    }, [load]);
    const verify = async (id) => {
        // 先把这一行标成「正在验证」，结果回来再换
        setView((current) => current && {
            ...current,
            connections: current.connections.map((conn) => (conn.id === id ? { ...conn, check: { kind: 'checking' } } : conn)),
        });
        try {
            setView(await checkConnection(id));
        }
        catch (error) {
            toast.error(error.message);
        }
    };
    const onCreated = async (id, withKey, next) => {
        setView(next);
        setOpen(null);
        toast.success(withKey ? '已添加；密钥进了 Keyring，正在验证' : '已添加；填上密钥才能用');
        if (withKey)
            setView(await checkConnection(id));
    };
    const rows = useMemo(() => view?.connections ?? [], [view]);
    if (!view)
        return _jsx("div", { className: "mt-6 text-2 text-ink-3", children: "\u6B63\u5728\u8BFB\u53D6\u2026" });
    const broken = Boolean(view.keyFile.problem);
    return (_jsxs("div", { "data-settings": "model", children: [_jsx("p", { className: "mt-1.5 text-2 leading-5 text-ink-2", children: "\u7528\u4F60\u81EA\u5DF1\u7684\u6A21\u578B\u670D\u52A1\uFF0C\u4E0D\u7528\u6CE8\u518C AIOS \u8D26\u53F7\u3002\u5728\u8FD9\u91CC\u586B\u7684\u5BC6\u94A5\u90FD\u5B58\u8FDB Keyring\u3002" }), broken ? _jsx(KeyringBroken, { view: view, onView: setView }) : null, _jsxs("section", { className: "mt-6", children: [_jsx("h2", { className: "text-3 font-medium text-ink", children: "\u6A21\u578B\u63D0\u4F9B\u5546" }), _jsxs("div", { className: "mt-2", children: [rows.length === 0 ? (_jsx("p", { className: "text-2 text-ink-3", children: "\u8FD8\u6CA1\u6709\u63A5\u4EFB\u4F55\u6A21\u578B\u3002\u63A5\u4E0A\u4E00\u4E2A\uFF0CKind \u624D\u80FD\u5F00\u59CB\u505A\u4E8B\u3002" })) : (_jsx("div", { className: "rounded-card border border-line", children: rows.map((conn, index) => (_jsxs("div", { className: cn(index > 0 && 'border-t border-line'), children: [_jsxs("div", { className: "flex items-center gap-3 px-4 py-3", children: [_jsx(StatusDot, { conn: conn, broken: broken }), _jsxs("div", { className: "min-w-0", children: [_jsx("div", { className: "text-3 text-ink", children: conn.name }), _jsx("div", { className: "mt-0.5 truncate text-1 text-ink-3", children: keyStatus(conn, broken).text })] }), _jsxs("div", { className: "ml-auto flex shrink-0 items-center gap-1", children: [conn.key.saved ? (_jsx(Btn, { kind: "quiet", disabled: broken || conn.check.kind === 'checking', title: broken ? '先让 Keyring 能读出来' : undefined, onClick: () => void verify(conn.id), children: conn.check.kind === 'checking' ? '正在验证…' : '验证' })) : null, _jsx(Btn, { kind: "plain", onClick: () => setOpen(open === conn.id ? null : conn.id), children: open === conn.id ? '收起' : '编辑' })] })] }), open === conn.id ? (_jsx(ConnectionEditor, { conn: conn, view: view, onDone: () => setOpen(null), onView: setView }, conn.id)) : null] }, conn.id))) })), connectedOnly && rows.some((conn) => KERNEL_PROVIDERS.has(conn.id)) ? null : open === 'add' ? (_jsx(AddCard, { view: view, onCreated: (id, withKey, next) => void onCreated(id, withKey, next), onCancel: () => setOpen(null) })) : (_jsxs(Btn, { kind: "plain", className: "mt-2.5 inline-flex items-center gap-1", onClick: () => setOpen('add'), children: [_jsx(Plus, { className: "size-3.5" }), "\u6DFB\u52A0\u6A21\u578B\u63D0\u4F9B\u5546"] }))] })] }), _jsx(KeyringSection, { view: view, onView: setView }), _jsx("p", { className: "mt-10 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 Keyring\uFF1AAIOS \u91CC\u6240\u6709\u51ED\u8BC1\u7684\u552F\u4E00\u53BB\u5904\u3002\u9875\u9762\u53EA\u770B\u5F97\u5230\u300C\u6709\u6CA1\u6709\u3001\u80FD\u4E0D\u80FD\u7528\u300D\uFF0C\u770B\u4E0D\u5230\u5BC6\u94A5\u672C\u8EAB\u3002" })] }));
}
