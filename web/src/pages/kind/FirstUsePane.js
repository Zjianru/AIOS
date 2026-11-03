import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 首次使用的 Kind：对话面板（brief §四，用户 2026-10-27 晚定稿；10 篇第 2 节）。
 *
 * 两步都在对话里完成，不跳设置页：
 * 1) 接模型：选提供方、填密钥。密钥交给 Keyring（和「设置 › 模型与连接」同一套：
 *    同一份提供方目录、同一个保存和验证）；「已保存」和「连接可用」分两步显示；
 * 2) 接好后 Kind 说一句，下面 3 张起手卡片（每张对应一个真实能力），点一张它说明这次按哪档做；
 *    再可选问一句「你主要用我做什么」，按回答推荐内置流程和角色——只推荐、不自动采用。
 * 打开开发开关「首次使用（空库）」时显示，数据从 api 取，不 import 样例。
 * 原型约定：以 sk-bad 开头的密钥模拟被服务商拒绝。
 */
import { useEffect, useState } from 'react';
import { Check, Eye, EyeOff, Loader2, X } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { useAppStore } from '@/app/store';
import { addConnection, checkConnection, getFirstUse, getModelSettings, KERNEL_PROVIDERS, syncKeyringEmpty, updateConnection } from '@/data/api';
import { useKindStore } from './kindStore';
import { cn } from '@/lib/utils';
function KindOrb({ size = 34 }) {
    return (_jsx("span", { className: "block shrink-0 rounded-full", style: {
            width: size,
            height: size,
            background: 'radial-gradient(circle at 35% 30%, var(--brand), var(--brand-soft))',
        } }));
}
/** 两步各一行：已保存 / 连接可用。 */
function StepLine({ state, children }) {
    return (_jsxs("div", { className: "flex items-start gap-2 text-2 leading-5", children: [_jsxs("span", { className: "mt-0.5 grid size-4 shrink-0 place-items-center", children: [state === 'ok' ? _jsx(Check, { className: "size-3.5", style: { color: 'var(--done)' } }) : null, state === 'fail' ? _jsx(X, { className: "size-3.5", style: { color: 'var(--fail)' } }) : null, state === 'run' ? _jsx(Loader2, { className: "size-3.5 animate-spin text-ink-3" }) : null, state === 'wait' ? _jsx("span", { className: "block size-1.5 rounded-full bg-line-2" }) : null] }), _jsx("span", { className: state === 'wait' ? 'text-ink-3' : state === 'fail' ? '' : 'text-ink', style: state === 'fail' ? { color: 'var(--fail)' } : undefined, children: children })] }));
}
function ConnectForm({ onConnected }) {
    const [catalog, setCatalog] = useState([]);
    const [provider, setProvider] = useState('');
    const [key, setKey] = useState('');
    const [shown, setShown] = useState(false);
    const [busy, setBusy] = useState(false);
    /** 这次提交以后的连接状态；没提交过是 null。 */
    const [conn, setConn] = useState(null);
    const active = catalog.find((item) => item.id === provider);
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    useEffect(() => {
        let alive = true;
        void getModelSettings().then((view) => {
            if (!alive)
                return;
            // 只显示接上内核的部分时，目录里只有内核会说的（D36：DeepSeek）
            const items = useAppStore.getState().connectedOnly
                ? view.catalog.filter((item) => KERNEL_PROVIDERS.has(item.id))
                : view.catalog;
            setCatalog(items);
            setProvider((current) => current || items[0]?.id || '');
        });
        return () => {
            alive = false;
        };
    }, []);
    const submit = async () => {
        if (!active || !key.trim())
            return;
        setBusy(true);
        try {
            const existing = (await getModelSettings()).connections.find((item) => item.id === active.id);
            const saved = existing
                ? await updateConnection(active.id, { endpoint: existing.endpoint, models: existing.models, key })
                : await addConnection({ source: 'catalog', id: active.id, name: active.name, protocol: active.protocol, endpoint: '', models: [], key });
            setKey('');
            setShown(false);
            setConn(saved.connections.find((item) => item.id === active.id) ?? null);
            const checked = await checkConnection(active.id);
            setConn(checked.connections.find((item) => item.id === active.id) ?? null);
        }
        catch (error) {
            toast.error(error.message);
        }
        finally {
            setBusy(false);
        }
    };
    const ok = conn?.check.kind === 'ok';
    const failed = conn?.check.kind === 'fail' ? conn.check.reason : null;
    return (_jsx("div", { className: "flex h-full items-center justify-center overflow-y-auto px-6", children: _jsxs("div", { className: "w-full max-w-[560px] py-10", "data-first-use": "connect", children: [_jsxs("div", { className: "flex items-center gap-2.5", children: [_jsx(KindOrb, {}), _jsx("span", { className: "text-3 text-ink-2", children: "Kind" })] }), _jsx("h1", { className: "mt-5 text-5 font-medium leading-7 text-ink", children: "\u5148\u63A5\u4E00\u4E2A\u6A21\u578B\uFF0C\u6211\u4EEC\u5C31\u80FD\u5F00\u59CB\u4E86" }), _jsx("p", { className: "mt-1 text-2 leading-5 text-ink-2", children: "\u7528\u4F60\u81EA\u5DF1\u7684\u6A21\u578B\u670D\u52A1\uFF0C\u4E0D\u7528\u6CE8\u518C AIOS \u8D26\u53F7\u3002\u5BC6\u94A5\u4EA4\u7ED9 Keyring \u52A0\u5BC6\u4FDD\u5B58\u5728\u8FD9\u53F0 Mac \u4E0A\uFF0C\u4FDD\u5B58\u4EE5\u540E\u9875\u9762\u3001\u65E5\u5FD7\u548C\u5BF9\u8BDD\u91CC\u90FD\u4E0D\u518D\u51FA\u73B0\u3002" }), _jsxs("div", { className: "mt-5 rounded-card border border-line-2 bg-raised/60 p-4", children: [_jsx("div", { className: "text-1 text-ink-3", children: "\u63D0\u4F9B\u65B9" }), _jsx("div", { className: "mt-1.5 flex flex-wrap gap-1.5", children: catalog.map((item) => (_jsx("button", { type: "button", disabled: busy || ok, onClick: () => {
                                    setProvider(item.id);
                                    setConn(null);
                                }, className: cn('rounded-btn border px-2.5 py-1 text-2 transition-colors disabled:cursor-not-allowed', item.id === provider
                                    ? 'border-ink-3 bg-hover text-ink'
                                    : 'border-line-2 text-ink-2 hover:bg-hover hover:text-ink disabled:hover:bg-transparent'), children: item.name }, item.id))) }), ok ? null : (_jsxs(_Fragment, { children: [_jsxs("label", { className: "mt-4 block text-1 text-ink-3", htmlFor: "first-use-key", children: [active?.name ?? '提供方', " \u7684 API \u5BC6\u94A5"] }), _jsxs("div", { className: "relative mt-1", children: [_jsx("input", { id: "first-use-key", type: shown ? 'text' : 'password', autoComplete: "new-password", spellCheck: false, value: key, disabled: busy, onChange: (event) => {
                                                setKey(event.target.value);
                                                setShown(false);
                                            }, onKeyDown: (event) => {
                                                if (event.key === 'Enter')
                                                    void submit();
                                            }, placeholder: failed ? '换一把再试' : '输入 API 密钥', className: cn('w-full rounded-btn border border-line-2 bg-pane px-2.5 py-1.5 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3', key && 'pr-9') }), key ? (_jsx("button", { type: "button", onClick: () => setShown((on) => !on), "aria-label": shown ? '隐藏密钥' : '显示密钥', className: "absolute right-1.5 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink", children: shown ? _jsx(EyeOff, { className: "size-3.5" }) : _jsx(Eye, { className: "size-3.5" }) })) : null] })] })), conn ? (_jsxs("div", { className: "mt-4 space-y-1", "data-first-use": "status", children: [_jsx(StepLine, { state: conn.key.saved ? 'ok' : 'wait', children: "\u5DF2\u4FDD\u5B58\u5230 Keyring" }), _jsx(StepLine, { state: ok ? 'ok' : failed ? 'fail' : 'run', children: ok ? '连接可用' : failed ? `连接不可用：${failed}` : `正在向 ${conn.name} 验证…` })] })) : null, _jsxs("div", { className: "mt-4 flex flex-wrap items-center gap-2", children: [ok ? (_jsx("button", { type: "button", onClick: onConnected, className: "rounded-btn bg-ink px-3 py-1.5 text-2 text-content transition-opacity hover:opacity-90", children: "\u5F00\u59CB" })) : (_jsx("button", { type: "button", disabled: !key.trim() || busy, onClick: () => void submit(), className: cn('rounded-btn px-3 py-1.5 text-2 transition-opacity', key.trim() && !busy ? 'bg-ink text-content hover:opacity-90' : 'cursor-not-allowed bg-hover text-ink-3'), children: busy ? '正在接…' : failed ? '再试一次' : '接上' })), connectedOnly ? null : (_jsx("button", { type: "button", onClick: () => go('settings:model'), className: "ml-auto text-1 text-ink-3 hover:text-ink", children: "\u8981\u63A5\u4E2D\u8F6C\u7AD9\u6216\u81EA\u5DF1\u90E8\u7F72\u7684\u63A5\u53E3\uFF1F\u53BB\u8BBE\u7F6E \u203A" }))] })] }), _jsx("p", { className: "mt-4 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u9996\u6B21\u4F7F\u7528\uFF1A\u6CA1\u63A5\u6A21\u578B\u4E4B\u524D\uFF0CKind \u53EA\u80FD\u505A\u8FD9\u4E00\u4EF6\u4E8B\u2014\u2014\u628A\u6A21\u578B\u63A5\u4E0A\u3002\u8FD9\u91CC\u5B58\u7684\u5BC6\u94A5\u548C\u300C\u8BBE\u7F6E \u203A \u6A21\u578B\u4E0E\u8FDE\u63A5\u300D\u662F\u540C\u4E00\u4EFD\u3002" })] }) }));
}
function StarterCard({ card, onPick }) {
    return (_jsxs("button", { type: "button", onClick: () => onPick(card), className: "flex h-full flex-col rounded-card border border-line bg-pane/60 p-3 text-left transition-colors hover:border-ink-3/60 hover:bg-raised", children: [_jsx("span", { className: "text-1 text-ink-3", children: card.ability }), _jsx("span", { className: "mt-1 text-3 font-medium leading-5 text-ink", children: card.title }), _jsx("span", { className: "mt-1 flex-1 text-1 leading-4 text-ink-3", children: card.prompt }), _jsxs("span", { className: "mt-2 flex items-center gap-1", children: [_jsxs("span", { className: "rounded-full bg-hover px-2 py-[1px] text-1 text-ink-3", children: ["\u6309 ", card.tier, " \u505A"] }), _jsx("span", { className: "ml-auto text-1 text-ink-3", children: "\u5F00\u59CB \u203A" })] })] }));
}
function ReadyStep({ data }) {
    const [picked, setPicked] = useState(null);
    const [answer, setAnswer] = useState(null);
    const option = data.options.find((item) => item.id === answer) ?? null;
    return (_jsxs("div", { className: "mx-auto w-full max-w-[680px] py-8", "data-first-use": "ready", children: [_jsxs("div", { className: "flex items-start gap-2.5", children: [_jsx(KindOrb, {}), _jsxs("div", { className: "min-w-0", children: [_jsx("div", { className: "text-1 text-ink-3", children: "Kind" }), _jsx("div", { className: "mt-1 text-4 leading-6 text-ink", children: "\u597D\uFF0C\u63A5\u4E0A\u4E86\uFF0C\u6211\u662F Kind\u3002\u4F60\u60F3\u8BA9\u6211\u5148\u505A\u4EC0\u4E48\uFF1F" }), _jsx("p", { className: "mt-1 text-2 leading-5 text-ink-2", children: "\u4E0B\u9762\u662F\u4E09\u4EF6\u73B0\u5728\u5C31\u80FD\u505A\u7684\u4E8B\u3002\u70B9\u4E00\u5F20\uFF0C\u6211\u5C31\u6309\u90A3\u4EF6\u4E8B\u5F00\u59CB\u3002" })] })] }), _jsx("div", { className: "mt-5 grid gap-2 sm:grid-cols-3", children: data.cards.map((card) => (_jsx(StarterCard, { card: card, onPick: setPicked }, card.id))) }), picked ? (_jsxs("div", { className: "mt-4 flex items-start gap-2.5", children: [_jsx(KindOrb, {}), _jsxs("div", { className: "min-w-0", children: [_jsx("div", { className: "text-1 text-ink-3", children: "Kind" }), _jsx("div", { className: "mt-1 text-3 leading-5 text-ink", children: picked.say }), _jsxs("div", { className: "mt-1.5 flex items-center gap-2", children: [_jsx("button", { type: "button", onClick: () => toast('（原型）这件事会开始；空库里先不起真任务'), className: "rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90", children: "\u5C31\u8FD9\u6837\u5F00\u59CB" }), _jsxs("span", { className: "text-1 text-ink-3", children: ["\u6309 ", picked.tier, " \u505A \u00B7 \u4E5F\u53EF\u4EE5\u76F4\u63A5\u5728\u4E0B\u9762\u8F93\u5165\u6846\u6539"] })] })] })] })) : null, _jsxs("div", { className: "mt-6 rounded-card border border-line bg-pane/40 p-3", children: [_jsx("div", { className: "text-2 text-ink-2", children: data.question }), _jsx("div", { className: "mt-2 flex flex-wrap gap-1.5", children: data.options.map((item) => (_jsx("button", { type: "button", onClick: () => setAnswer(item.id === answer ? null : item.id), className: cn('rounded-full border px-2.5 py-1 text-2 transition-colors', item.id === answer
                                ? 'border-ink-3 bg-hover text-ink'
                                : 'border-line-2 text-ink-2 hover:bg-hover hover:text-ink'), children: item.label }, item.id))) }), option ? (_jsxs("div", { className: "mt-3 border-t border-line pt-2.5", "data-first-use": "recommend", children: [_jsxs("div", { className: "text-2 text-ink", children: ["\u6309\u300C", option.label, "\u300D\u63A8\u8350\uFF1A"] }), _jsxs("ul", { className: "mt-1 space-y-0.5 text-2 leading-5 text-ink-2", children: [option.recommend.flows.map((flow) => (_jsxs("li", { children: ["\u5185\u7F6E\u6D41\u7A0B \u00B7 ", _jsx("span", { className: "text-ink", children: flow.name }), "\uFF08", flow.tier, "\uFF09\uFF1A", flow.desc] }, flow.name))), option.recommend.roles.map((role) => (_jsxs("li", { children: ["\u89D2\u8272 \u00B7 ", _jsx("span", { className: "text-ink", children: role.name }), "\uFF1A", role.why] }, role.name)))] }), _jsxs("div", { className: "mt-2 flex items-center gap-2", children: [_jsx("button", { type: "button", onClick: () => toast('（原型）已采用推荐；采用后能在「能力 → 流程」里看到'), className: "rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90", children: "\u91C7\u7528\u63A8\u8350" }), _jsx("button", { type: "button", onClick: () => setAnswer(null), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u5148\u4E0D\u7528" }), _jsx("span", { className: "text-1 text-ink-3", children: "\u53EA\u63A8\u8350\uFF0C\u4E0D\u81EA\u52A8\u91C7\u7528" })] })] })) : (_jsx("p", { className: "mt-1.5 text-1 text-ink-3", children: "\u53EA\u63A8\u8350\uFF0C\u4E0D\u81EA\u52A8\u91C7\u7528\uFF1B\u4E4B\u540E\u968F\u65F6\u80FD\u5728\u300C\u80FD\u529B \u2192 \u6D41\u7A0B\u300D\u91CC\u6539\u3002" }))] }), _jsx("p", { className: "mt-8 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u9996\u6B21\u4F7F\u7528\uFF1A\u8D77\u624B\u5361\u7247\u5BF9\u5E94\u771F\u5B9E\u80FD\u529B\uFF0C\u4E0D\u505A\u529F\u80FD\u5BFC\u89C8\uFF1B\u63A8\u8350\u53EA\u662F\u63A8\u8350\uFF0C\u4E0D\u81EA\u52A8\u6539\u8BBE\u7F6E\u3002" })] }));
}
export function FirstUsePane() {
    const firstUse = useAppStore((state) => state.firstUse);
    syncKeyringEmpty(firstUse);
    const [data, setData] = useState(null);
    const [connected, setConnected] = useState(false);
    useEffect(() => {
        let alive = true;
        void (async () => {
            const next = await getFirstUse();
            if (alive)
                setData(next);
        })();
        return () => {
            alive = false;
        };
    }, []);
    if (!data) {
        return _jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u51C6\u5907\u9996\u6B21\u4F7F\u7528\u2026" });
    }
    return (_jsx("div", { className: "flex h-full min-h-0 flex-col overflow-y-auto bg-content", "data-pane": "conversation", children: connected ? (_jsx(ReadyStep, { data: data })) : (_jsx(ConnectForm, { onConnected: () => {
                // 只显示接上内核的部分：没有起手卡片（任务、流程还没接上），直接进空的 Kind 对话
                if (useAppStore.getState().connectedOnly) {
                    useKindStore.getState().demo('empty');
                    useAppStore.getState().setFirstUse(false);
                    return;
                }
                setConnected(true);
            } })) }));
}
