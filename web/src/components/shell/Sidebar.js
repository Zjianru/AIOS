import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 侧栏：220px，可收到 56px 图标列（layout-motion-v1.md §二）。
 * 用 shadcn 的 Sidebar（collapsible="icon"），只改样式 token 与宽度变量，行为不动。
 */
import { Clock3, FileText, FolderKanban, Layers2, Moon, Settings, Sparkles, Sun, Users } from 'lucide-react';
import { Sidebar as ShadcnSidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupLabel, SidebarHeader, SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarRail, } from '@/components/ui/sidebar';
import { useAppStore } from '@/app/store';
import { go } from '@/app/navigate';
import { navOf } from '@/app/route';
import { cn } from '@/lib/utils';
const MAIN_NAV = [
    { id: 'now', label: '现在', route: 'now', icon: Clock3 },
    { id: 'kind', label: 'Kind', route: 'kind', icon: Sparkles },
    { id: 'work', label: '工作', route: 'work', icon: FolderKanban },
];
const OTHER_NAV = [
    { id: 'team', label: '团队', route: 'team', icon: Users },
    { id: 'ability', label: '能力', route: 'ability', icon: Layers2 },
    { id: 'lib', label: '资料', route: 'lib', icon: FileText },
    { id: 'memory', label: '记忆', route: 'memory', icon: FileText },
];
const PROJECTS = [
    { id: 'wsp', label: 'web-search-pro 研究', code: 'W' },
    { id: 'news', label: 'news-intelligence', code: 'N' },
    { id: 'daily', label: '日常', code: '日' },
];
function TrafficLights() {
    return (_jsxs("div", { className: "flex items-center gap-[8px] px-1 pt-1", children: [_jsx("i", { className: "size-[11px] rounded-full bg-[#ff5f57]" }), _jsx("i", { className: "size-[11px] rounded-full bg-[#febc2e]" }), _jsx("i", { className: "size-[11px] rounded-full bg-[#28c840]" })] }));
}
function Item({ item, active }) {
    const Icon = item.icon;
    return (_jsx(SidebarMenuItem, { children: _jsxs(SidebarMenuButton, { isActive: active, tooltip: item.label, onClick: () => go(item.route), className: cn('h-8 gap-2.5 rounded-btn px-2 text-3 text-ink-2 transition-colors', active && 'bg-active text-ink', 'hover:bg-hover hover:text-ink'), children: [_jsx(Icon, { className: "size-4 shrink-0" }), _jsx("span", { className: "truncate group-data-[collapsible=icon]:hidden", children: item.label }), item.id === 'now' ? (_jsxs("span", { className: "ml-auto flex items-center gap-[3px] group-data-[collapsible=icon]:hidden", children: [_jsx("i", { className: "size-1 rounded-full bg-fail" }), _jsx("i", { className: "size-1 rounded-full bg-wait" })] })) : null] }) }));
}
function SidebarInner({ route }) {
    const active = navOf(route);
    const theme = useAppStore((s) => s.theme);
    const toggleTheme = useAppStore((s) => s.toggleTheme);
    // 只显示接上内核的部分（D46）：只有 Kind 和设置（外观是本地开关，留着）。
    const connectedOnly = useAppStore((s) => s.connectedOnly);
    const mainNav = connectedOnly ? MAIN_NAV.filter((item) => item.id === 'kind') : MAIN_NAV;
    return (_jsxs(ShadcnSidebar, { collapsible: "icon", className: "border-r border-line bg-side backdrop-blur-xl", style: { ['--sidebar-width']: '220px', ['--sidebar-width-icon']: '56px' }, children: [_jsxs(SidebarHeader, { className: "gap-2 px-2.5 pt-3", children: [_jsx(TrafficLights, {}), connectedOnly ? null : (_jsx(SidebarMenu, { children: _jsx(SidebarMenuItem, { children: _jsxs(SidebarMenuButton, { tooltip: "\u641C\u7D22\uFF08\u7B2C 3 \u671F\uFF09", className: "h-8 gap-2.5 rounded-btn border border-line bg-pane/60 px-2 text-3 text-ink-3", "aria-disabled": true, children: [_jsxs("svg", { viewBox: "0 0 24 24", className: "size-4 shrink-0", fill: "none", stroke: "currentColor", strokeWidth: "1.8", children: [_jsx("circle", { cx: "11", cy: "11", r: "7" }), _jsx("path", { d: "M20 20l-4-4" })] }), _jsx("span", { className: "truncate group-data-[collapsible=icon]:hidden", children: "\u641C\u7D22" }), _jsx("kbd", { className: "ml-auto brand-type text-1 text-ink-3 group-data-[collapsible=icon]:hidden", children: "\u2318K" })] }) }) }))] }), _jsxs(SidebarContent, { className: "gap-0 px-2", children: [_jsx(SidebarGroup, { className: "py-1", children: _jsx(SidebarMenu, { children: mainNav.map((item) => (_jsx(Item, { item: item, active: active === item.id }, item.id))) }) }), connectedOnly ? null : (_jsxs(_Fragment, { children: [_jsxs(SidebarGroup, { className: "py-1", children: [_jsx(SidebarGroupLabel, { className: "h-6 px-2 text-1 font-normal text-ink-3 group-data-[collapsible=icon]:hidden", children: "\u9879\u76EE" }), _jsx(SidebarMenu, { children: PROJECTS.map((project) => (_jsx(SidebarMenuItem, { children: _jsxs(SidebarMenuButton, { isActive: active === 'project:' + project.id, tooltip: project.label, onClick: () => go('project:' + project.id), className: cn('h-8 gap-2.5 rounded-btn px-2 text-3 text-ink-2 transition-colors hover:bg-hover hover:text-ink', active === 'project:' + project.id && 'bg-active text-ink'), children: [_jsx("span", { className: "grid size-4 shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: {
                                                            background: project.id === 'wsp' ? 'var(--line-w)' : project.id === 'news' ? 'var(--line-n)' : 'var(--line-d)',
                                                        }, children: project.code }), _jsx("span", { className: "truncate group-data-[collapsible=icon]:hidden", children: project.label })] }) }, project.id))) })] }), _jsx(SidebarGroup, { className: "py-1", children: _jsx(SidebarMenu, { children: OTHER_NAV.map((item) => (_jsx(Item, { item: item, active: active === item.id }, item.id))) }) })] }))] }), _jsx(SidebarFooter, { className: "gap-0.5 px-2 pb-3", children: _jsxs(SidebarMenu, { children: [_jsx(SidebarMenuItem, { children: _jsxs(SidebarMenuButton, { tooltip: theme === 'dark' ? '换浅色（外观）' : '换深色（外观）', onClick: toggleTheme, className: "h-8 gap-2.5 rounded-btn px-2 text-3 text-ink-2 hover:bg-hover hover:text-ink", children: [theme === 'dark' ? _jsx(Moon, { className: "size-4 shrink-0" }) : _jsx(Sun, { className: "size-4 shrink-0" }), _jsx("span", { className: "truncate group-data-[collapsible=icon]:hidden", children: "\u5916\u89C2" })] }) }), _jsx(SidebarMenuItem, { children: _jsxs(SidebarMenuButton, { tooltip: "\u8BBE\u7F6E", isActive: active === 'settings', onClick: () => go(connectedOnly ? 'settings:model' : 'settings'), className: "h-8 gap-2.5 rounded-btn px-2 text-3 text-ink-2 hover:bg-hover hover:text-ink", children: [_jsx(Settings, { className: "size-4 shrink-0" }), _jsx("span", { className: "truncate group-data-[collapsible=icon]:hidden", children: "\u8BBE\u7F6E" })] }) })] }) }), _jsx(SidebarRail, {})] }));
}
export function Sidebar({ route }) {
    return _jsx(SidebarInner, { route: route });
}
