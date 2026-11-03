import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 应用外壳（layout-motion-v1.md §二）：
 * 窗口 12px 圆角、侧栏 220/56、工具栏 44、每个场景一个 dock 根。
 */
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useLocation } from 'react-router';
import { SidebarProvider } from '@/components/ui/sidebar';
import { Sidebar } from './Sidebar';
import { Toolbar } from './Toolbar';
import { SceneDock } from './SceneDock';
import { parseRoute, sceneOf } from '@/app/route';
import { sceneDef } from '@/app/scenes';
import { useAppStore } from '@/app/store';
import { useKindStore } from '@/pages/kind/kindStore';
import { useSystemReducedMotion } from '@/app/useSystemReducedMotion';
import { registerHook, installTestHooks } from '@/app/testHooks';
import { syncRoute } from '@/app/routeStore';
import { currentRoute, go } from '@/app/navigate';
import { markRoutePainted, markRouteStart } from '@/lib/perf';
import { KEYRING_PROBLEM, setKeyringProblem, setMockLatency } from '@/data/api';
import { KindScene } from '@/pages/kind/KindScene';
import { AbilityScene } from '@/pages/ability/AbilityScene';
import { NowScene } from '@/pages/now/NowScene';
import { EmptyNow } from '@/pages/now/EmptyNow';
import { WorkScene } from '@/pages/work/WorkScene';
import { ProjectScene } from '@/pages/project/ProjectScene';
import { ReadScene } from '@/pages/read/ReadScene';
import { TaskScene } from '@/pages/task/TaskScene';
import { TraceScene } from '@/pages/task/TraceScene';
import { SettingsScene } from '@/pages/settings/SettingsScene';
import { PeriodPage } from '@/pages/placeholder/PeriodPage';
export function AppShell() {
    const location = useLocation();
    const route = useMemo(() => parseRoute(location.pathname + location.search + (location.hash ? '' : '')), [location.pathname, location.search]);
    const scene = sceneOf(route);
    const def = sceneDef(scene);
    const connectedOnly = useAppStore((s) => s.connectedOnly);
    const kernel = useAppStore((s) => s.kernel);
    // 只显示接上内核的部分（D46）：Kind 只有对话一栏；接不上内核时也只留一栏放说明。
    const soloKind = scene === 'kind' && (connectedOnly || kernel !== 'ok');
    const panes = useMemo(() => (soloKind ? def.panes.filter((pane) => pane.id === 'conversation') : def.panes), [soloKind, def]);
    const dockKey = soloKind ? scene + ':solo' : scene;
    const theme = useAppStore((s) => s.theme);
    const latency = useAppStore((s) => s.latency);
    const reducedPref = useAppStore((s) => s.reducedMotion);
    const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed);
    const setSidebarCollapsed = useAppStore((s) => s.setSidebarCollapsed);
    const systemReduced = useSystemReducedMotion();
    const reduced = reducedPref ?? systemReduced;
    const [resetToken, setResetToken] = useState(0);
    useEffect(() => {
        document.documentElement.dataset.theme = theme;
        if (reduced)
            document.documentElement.dataset.reducedMotion = 'true';
        else
            delete document.documentElement.dataset.reducedMotion;
    }, [theme, reduced]);
    // 只显示接上内核的部分：其他场景还没接上，回到 Kind。
    useEffect(() => {
        if (connectedOnly && scene !== 'kind' && scene !== 'settings')
            go('kind');
    }, [connectedOnly, scene]);
    // 假数据层的延迟开关（设置里的开发开关）
    useEffect(() => {
        setMockLatency(latency);
    }, [latency]);
    // 路由切换：起点 mark + 画完 mark（Kind 页自己会再打细的 mark）
    useEffect(() => {
        syncRoute(route);
    }, [route]);
    useEffect(() => {
        markRouteTracked(route.raw);
        return () => {
            clearTimeout(paintTimer);
        };
    }, [route.raw]);
    const resetLayout = useCallback(() => {
        useAppStore.getState().resetLayout(scene);
        setResetToken((n) => n + 1);
    }, [scene]);
    // 面板内容自己从 store 取路由（dockview 面板不会跟着父组件重渲染）
    const renderPane = useCallback((paneId) => {
        if (scene === 'kind')
            return _jsx(KindScene, { pane: paneId });
        if (scene === 'ability')
            return _jsx(AbilityScene, {});
        if (scene === 'now')
            return _jsx(NowScene, {});
        if (scene === 'work')
            return _jsx(WorkScene, { pane: paneId });
        if (scene === 'project' && route.kind === 'project')
            return _jsx(ProjectScene, { pane: paneId });
        if (scene === 'read')
            return _jsx(ReadScene, { pane: paneId });
        if (scene === 'task')
            return _jsx(TaskScene, {});
        if (scene === 'trace')
            return _jsx(TraceScene, {});
        if (scene === 'empty' && route.id === 'now')
            return _jsx(EmptyNow, {});
        if (scene === 'settings' && route.kind === 'settings')
            return _jsx(SettingsScene, {});
        return _jsx(PeriodPage, {});
    }, [scene, route.id]);
    useEffect(() => {
        installTestHooks();
        registerHook('go', ((next) => go(next)));
        registerHook('currentRoute', (() => currentRoute()));
        registerHook('setTheme', ((theme) => useAppStore.getState().setTheme(theme)));
        registerHook('setReducedMotion', ((on) => useAppStore.getState().setReducedMotion(on)));
        registerHook('setSidebarCollapsed', ((collapsed) => useAppStore.getState().setSidebarCollapsed(collapsed)));
        registerHook('setDevStressFlow', ((on) => useAppStore.getState().setDevStressFlow(on)));
        registerHook('setFirstUse', ((on) => useAppStore.getState().setFirstUse(on)));
        registerHook('setConnectedOnly', ((on) => useAppStore.getState().setConnectedOnly(on)));
        registerHook('setKernel', ((state) => useAppStore.getState().setKernel(state)));
        registerHook('kindDemo', ((name) => useKindStore.getState().demo(name)));
        registerHook('setKeyringBroken', ((on) => {
            setKeyringProblem(on ? KEYRING_PROBLEM : null);
            useAppStore.getState().setKeyringBroken(on);
        }));
    }, []);
    return (_jsx("div", { className: "h-full w-full p-3 sm:p-5", children: _jsx("div", { 
            // transform 让这个窗口成为 fixed 子元素的包含块：shadcn 的侧栏是 fixed 的，
            // 而原型是一个带外边距的「窗口」，侧栏要贴窗口而不是贴视口。
            className: "relative mx-auto flex h-full w-full max-w-[1920px] overflow-hidden rounded-window border border-line bg-content text-ink shadow-[var(--shadow-pop)] [transform:translateZ(0)]", "data-route": route.raw, "data-scene-key": scene, children: _jsxs(SidebarProvider, { className: "h-full min-h-0 w-full", open: !sidebarCollapsed, onOpenChange: (open) => setSidebarCollapsed(!open), style: { ['--sidebar-width']: '220px', ['--sidebar-width-icon']: '56px' }, children: [_jsx(Sidebar, { route: route }), _jsxs("div", { className: "flex min-w-0 flex-1 flex-col", children: [_jsx(Toolbar, { route: route, onResetLayout: resetLayout }), _jsx("main", { className: "min-h-0 flex-1 overflow-hidden", children: _jsx(SceneDock, { scene: dockKey, panes: panes, renderPane: renderPane, resetToken: resetToken }, dockKey) })] })] }) }) }));
}
let paintTimer;
function markRouteTracked(raw) {
    markRouteStart(raw);
    clearTimeout(paintTimer);
    paintTimer = setTimeout(() => markRoutePainted(raw), 0);
}
