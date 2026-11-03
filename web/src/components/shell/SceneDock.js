import { Fragment as _Fragment, jsx as _jsx } from "react/jsx-runtime";
/**
 * 每个场景一个 dock 根（layout-motion-v1.md §一）。
 * - 默认布局按场景给；用户改过的布局用 dockview 的 toJSON / fromJSON 按场景存。
 * - 窗口太窄放不下当前分栏时，把面板并到同一个分组里（自动收成标签页）；
 *   放宽后原样恢复。这一步不写进用户存的布局（§二：算窗口适配，不是布局变化）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { DockviewReact } from 'dockview-react';
import { useAppStore } from '@/app/store';
const SPLITTER_WIDTH = 6;
function buildDefault(api, panes) {
    const [first, ...rest] = panes;
    api.addPanel({ id: first.id, component: first.id, title: first.title });
    for (const pane of rest) {
        api.addPanel({
            id: pane.id,
            component: pane.id,
            title: pane.title,
            position: { referencePanel: first.id, direction: 'right' },
            initialWidth: pane.defaultWidth ?? pane.minWidth,
        });
    }
}
/** 存过的布局是不是还属于当前这套面板：面板 id 变过（第 2 期把「在做的事」换成右栏）就整个弃用，回默认布局。 */
function layoutFits(stored, panes) {
    const known = new Set(panes.map((pane) => pane.id));
    const panels = stored.panels ?? {};
    const ids = Object.keys(panels);
    return ids.length > 0 && ids.every((id) => known.has(id));
}
export function SceneDock({ scene, panes, renderPane, resetToken, onNarrowChange }) {
    const hostRef = useRef(null);
    const apiRef = useRef(null);
    const [narrow, setNarrow] = useState(false);
    const narrowRef = useRef(false);
    const savedRef = useRef(null);
    const applyingRef = useRef(false);
    const saveTimer = useRef(null);
    const renderRef = useRef(renderPane);
    renderRef.current = renderPane;
    const paneKey = panes.map((p) => p.id).join(',');
    // dockview 的 components 必须稳定，否则会反复重注册；内容走 renderRef。
    const components = useMemo(() => {
        const map = {};
        for (const id of paneKey.split(','))
            map[id] = () => _jsx(_Fragment, { children: renderRef.current(id) });
        return map;
    }, [paneKey]);
    const handleReady = useCallback((event) => {
        const api = event.api;
        apiRef.current = api;
        applyingRef.current = true;
        const stored = useAppStore.getState().layouts[scene];
        let used = false;
        if (stored && layoutFits(stored, panes)) {
            try {
                api.fromJSON(stored);
                used = true;
            }
            catch {
                used = false;
            }
        }
        if (!used)
            buildDefault(api, panes);
        for (const pane of panes) {
            if (!api.getPanel(pane.id)) {
                api.addPanel({ id: pane.id, component: pane.id, title: pane.title });
            }
        }
        applyingRef.current = false;
        api.onDidLayoutChange(() => {
            if (applyingRef.current || narrowRef.current)
                return;
            if (saveTimer.current)
                window.clearTimeout(saveTimer.current);
            saveTimer.current = window.setTimeout(() => {
                const target = apiRef.current;
                if (!target || narrowRef.current)
                    return;
                useAppStore.getState().saveLayout(scene, target.toJSON());
            }, 400);
        });
    }, [scene, panes]);
    // 窄窗判断：放不下各面板最小宽度之和时收起。
    useEffect(() => {
        const host = hostRef.current;
        if (!host)
            return;
        const required = panes.reduce((sum, pane) => sum + pane.minWidth, 0) + Math.max(0, panes.length - 1) * SPLITTER_WIDTH;
        const ro = new ResizeObserver((entries) => {
            const width = entries[0]?.contentRect.width ?? host.clientWidth;
            const next = width < required;
            if (next === narrowRef.current)
                return;
            narrowRef.current = next;
            setNarrow(next);
            onNarrowChange?.(next);
        });
        ro.observe(host);
        return () => ro.disconnect();
    }, [panes, onNarrowChange]);
    // 收起 / 恢复。
    useEffect(() => {
        const api = apiRef.current;
        if (!api)
            return;
        if (narrow) {
            if (!savedRef.current)
                savedRef.current = api.toJSON();
            const groups = api.groups;
            if (groups.length < 2)
                return;
            const target = groups[0];
            applyingRef.current = true;
            for (const panel of api.panels) {
                if (panel.api.group.id !== target.id)
                    panel.api.moveTo({ group: target, skipSetActive: true });
            }
            applyingRef.current = false;
            return;
        }
        const saved = savedRef.current;
        savedRef.current = null;
        if (!saved)
            return;
        applyingRef.current = true;
        try {
            api.fromJSON(saved);
        }
        catch {
            api.clear();
            buildDefault(api, panes);
        }
        applyingRef.current = false;
    }, [narrow, panes]);
    // 重置我的布局
    useEffect(() => {
        if (resetToken === 0)
            return;
        const api = apiRef.current;
        if (!api)
            return;
        applyingRef.current = true;
        try {
            api.clear();
            buildDefault(api, panes);
        }
        finally {
            applyingRef.current = false;
        }
    }, [resetToken, panes]);
    const theme = useAppStore((s) => s.theme);
    return (_jsx("div", { ref: hostRef, className: "h-full w-full", "data-scene": scene, "data-narrow": narrow ? 'true' : 'false', children: _jsx(DockviewReact, { className: theme === 'dark' ? 'dv-theme dockview-theme-dark' : 'dv-theme dockview-theme-light', components: components, onReady: handleReady }) }));
}
