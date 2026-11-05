/**
 * 每个场景一个 dock 根（layout-motion-v1.md §一）。
 * - 默认布局按场景给；用户改过的布局用 dockview 的 toJSON / fromJSON 按场景存。
 * - 窗口太窄放不下当前分栏时，把面板并到同一个分组里（自动收成标签页）；
 *   放宽后原样恢复。这一步不写进用户存的布局（§二：算窗口适配，不是布局变化）。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { DockviewReact } from 'dockview-react';
import type { DockviewApi, DockviewReadyEvent, SerializedDockview } from 'dockview-react';
import type { PaneDef } from '@/app/scenes';
import { useAppStore } from '@/app/store';

interface SceneDockProps {
  scene: string;
  panes: PaneDef[];
  renderPane: (paneId: string) => React.ReactNode;
  /** 换一个值就回到默认布局（工具栏「重置我的布局」）。 */
  resetToken: number;
  onNarrowChange?: (narrow: boolean) => void;
}

const SPLITTER_WIDTH = 6;

/** T009.5：dockview 的读屏播报改成中文，覆盖它内置的全部英文条目（DockviewMessages）。 */
const EDGE_NAMES: Record<string, string> = {
  top: '上方',
  bottom: '下方',
  left: '左侧',
  right: '右侧',
  center: '中间',
};

const DOCKVIEW_MESSAGES = {
  panelOpened: (title: string) => '打开了 ' + title,
  panelClosed: (title: string) => '关闭了 ' + title,
  groupMaximized: (title: string) => '放大了 ' + title,
  groupRestored: (title: string) => '恢复了 ' + title,
  groupFloated: (title: string) => '浮出了 ' + title,
  groupDocked: (title: string) => '停靠了 ' + title,
  groupPoppedOut: (title: string) => '弹出了 ' + title,
  closeTab: (title: string) => '关闭 ' + title,
  closeTabPlain: () => '关闭这个面板',
  movePickTarget: (source: string, target: string, current: number, total: number) =>
    '要移动 ' + source + '：目标是 ' + target + '（第 ' + current + ' 个，共 ' + total + ' 个），按回车放下，按 Esc 取消',
  movePickEdge: (position: string, target: string) =>
    '要移动：' + (EDGE_NAMES[position] ?? position) + ' ' + target + '，按回车放下，按 Esc 取消',
  moveCommitted: (source: string, target: string, position: string) =>
    '已把 ' + source + ' 移到 ' + target + ' 的 ' + (EDGE_NAMES[position] ?? position),
  moveCancelled: () => '移动已取消',
  moveNotAllowed: () => '这里不能放',
  moveFloated: (source: string) => '已把 ' + source + ' 浮出来',
};

function buildDefault(api: DockviewApi, panes: PaneDef[]): void {
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
function layoutFits(stored: SerializedDockview, panes: PaneDef[]): boolean {
  const known = new Set(panes.map((pane) => pane.id));
  const panels = (stored as { panels?: Record<string, unknown> }).panels ?? {};
  const ids = Object.keys(panels);
  return ids.length > 0 && ids.every((id) => known.has(id));
}

export function SceneDock({ scene, panes, renderPane, resetToken, onNarrowChange }: SceneDockProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const apiRef = useRef<DockviewApi | null>(null);
  const [narrow, setNarrow] = useState(false);
  const narrowRef = useRef(false);
  const savedRef = useRef<SerializedDockview | null>(null);
  const applyingRef = useRef(false);
  const saveTimer = useRef<number | null>(null);
  const renderRef = useRef(renderPane);
  renderRef.current = renderPane;

  const paneKey = panes.map((p) => p.id).join(',');
  // dockview 的 components 必须稳定，否则会反复重注册；内容走 renderRef。
  const components = useMemo(() => {
    const map: Record<string, () => React.ReactElement> = {};
    for (const id of paneKey.split(',')) map[id] = () => <>{renderRef.current(id)}</>;
    return map;
  }, [paneKey]);

  const handleReady = useCallback(
    (event: DockviewReadyEvent) => {
      const api = event.api;
      apiRef.current = api;
      applyingRef.current = true;
      const stored = useAppStore.getState().layouts[scene] as SerializedDockview | undefined;
      let used = false;
      if (stored && layoutFits(stored, panes)) {
        try {
          api.fromJSON(stored);
          used = true;
        } catch {
          used = false;
        }
      }
      if (!used) buildDefault(api, panes);
      for (const pane of panes) {
        if (!api.getPanel(pane.id)) {
          api.addPanel({ id: pane.id, component: pane.id, title: pane.title });
        }
      }
      applyingRef.current = false;
      api.onDidLayoutChange(() => {
        if (applyingRef.current || narrowRef.current) return;
        if (saveTimer.current) window.clearTimeout(saveTimer.current);
        saveTimer.current = window.setTimeout(() => {
          const target = apiRef.current;
          if (!target || narrowRef.current) return;
          useAppStore.getState().saveLayout(scene, target.toJSON());
        }, 400);
      });
    },
    [scene, panes],
  );

  // 窄窗判断：放不下各面板最小宽度之和时收起。
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const required =
      panes.reduce((sum, pane) => sum + pane.minWidth, 0) + Math.max(0, panes.length - 1) * SPLITTER_WIDTH;
    const ro = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? host.clientWidth;
      const next = width < required;
      if (next === narrowRef.current) return;
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
    if (!api) return;
    if (narrow) {
      if (!savedRef.current) savedRef.current = api.toJSON();
      const groups = api.groups;
      if (groups.length < 2) return;
      const target = groups[0];
      applyingRef.current = true;
      for (const panel of api.panels) {
        if (panel.api.group.id !== target.id) panel.api.moveTo({ group: target, skipSetActive: true });
      }
      applyingRef.current = false;
      return;
    }
    const saved = savedRef.current;
    savedRef.current = null;
    if (!saved) return;
    applyingRef.current = true;
    try {
      api.fromJSON(saved);
    } catch {
      api.clear();
      buildDefault(api, panes);
    }
    applyingRef.current = false;
  }, [narrow, panes]);

  // 重置我的布局
  useEffect(() => {
    if (resetToken === 0) return;
    const api = apiRef.current;
    if (!api) return;
    applyingRef.current = true;
    try {
      api.clear();
      buildDefault(api, panes);
    } finally {
      applyingRef.current = false;
    }
  }, [resetToken, panes]);

  const theme = useAppStore((s) => s.theme);

  return (
    <div ref={hostRef} className="h-full w-full" data-scene={scene} data-narrow={narrow ? 'true' : 'false'}>
      <DockviewReact
        className={theme === 'dark' ? 'dv-theme dockview-theme-dark' : 'dv-theme dockview-theme-light'}
        components={components}
        messages={DOCKVIEW_MESSAGES}
        onReady={handleReady}
      />
    </div>
  );
}
