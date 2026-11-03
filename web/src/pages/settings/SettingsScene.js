import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 设置场景（#settings、#settings:<分组>）。
 *
 * 分组照 v13：外观、模型与连接、Kind、执行、通知、隐私与更新、监控、关于。
 * 现在只做了「模型与连接」（它后面就是 Keyring，用户 2026-11-01 定：设置页现在就做，D47）；
 * 其余分组仍是第 3 期，先放一行说明。默认落到「外观」，照 ROUTES.md。
 */
import { go } from '@/app/navigate';
import { SETTINGS_GROUPS } from '@/app/route';
import { useRouteStore } from '@/app/routeStore';
import { cn } from '@/lib/utils';
import { useAppStore } from '@/app/store';
import { ModelSettings } from './ModelSettings';
/** 已经做出来的分组；其余仍是第 3 期。 */
const BUILT = new Set(['model']);
const GROUPS = SETTINGS_GROUPS.map((group) => ({ ...group, built: BUILT.has(group.id) }));
export function SettingsScene() {
    const route = useRouteStore((state) => state.route);
    // 只显示接上内核的部分（D46）：设置里只有「模型与连接」。
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    const groups = connectedOnly ? GROUPS.filter((group) => group.built) : GROUPS;
    const current = groups.find((group) => group.id === route.id) ?? groups[0];
    return (_jsxs("div", { className: "flex h-full min-h-0", "data-scene-page": "settings", children: [_jsxs("nav", { className: "w-[184px] shrink-0 border-r border-line px-2 py-4", "aria-label": "\u8BBE\u7F6E\u5206\u7EC4", children: [_jsx("div", { className: "px-2 pb-2 text-1 text-ink-3", children: "\u8BBE\u7F6E" }), groups.map((group) => (_jsxs("button", { type: "button", onClick: () => go('settings:' + group.id), "aria-current": group.id === current.id ? 'page' : undefined, className: cn('flex w-full items-center rounded-btn px-2 py-1.5 text-left text-3 transition-colors', group.id === current.id ? 'bg-hover text-ink' : 'text-ink-2 hover:bg-hover hover:text-ink'), children: [group.name, group.built ? null : _jsx("span", { className: "ml-auto text-1 text-ink-3", children: "\u7B2C 3 \u671F" })] }, group.id)))] }), _jsx("div", { className: "min-w-0 flex-1 overflow-y-auto", children: _jsxs("div", { className: "mx-auto w-full max-w-[720px] px-8 py-8", children: [_jsx("h1", { className: "text-5 font-medium text-ink", children: current.name }), current.id === 'model' ? (_jsx(ModelSettings, {})) : (_jsxs("p", { className: "mt-3 text-3 leading-6 text-ink-2", children: ["\u8FD9\u4E00\u7EC4\u6392\u5728\u7B2C 3 \u671F\uFF0C\u6837\u5B50\u89C1 v13 \u539F\u578B\u7684\u300C\u8BBE\u7F6E \u203A ", current.name, "\u300D\u3002"] }))] }) })] }));
}
