/**
 * 应用外壳（layout-motion-v1.md §二）：
 * 窗口 12px 圆角、侧栏 220/56、工具栏 44、每个场景一个 dock 根。
 */

import { useCallback, useEffect, useMemo } from 'react';
import { useLocation } from 'react-router';
import { SidebarProvider } from '@/components/ui/sidebar';
import { Sidebar } from './Sidebar';
import { Toolbar } from './Toolbar';
import { SceneDock } from './SceneDock';
import { parseRoute, sceneOf } from '@/app/route';
import { sceneDef } from '@/app/scenes';
import { useAppStore } from '@/app/store';
import { useSystemReducedMotion } from '@/app/useSystemReducedMotion';
import { syncRoute } from '@/app/routeStore';
import { go } from '@/app/navigate';
import { markRoutePainted, markRouteStart } from '@/lib/perf';

import { KindScene } from '@/pages/kind/KindScene';
import { AbilityScene } from '@/pages/ability/AbilityScene';
import { NowScene } from '@/pages/now/NowScene';
import { EmptyNow } from '@/pages/now/EmptyNow';
import { WorkScene } from '@/pages/work/WorkScene';
import { ProjectScene } from '@/pages/project/ProjectScene';
import { ReadScene } from '@/pages/read/ReadScene';
import { TaskScene } from '@/pages/task/TaskScene';
import { TraceScene } from '@/pages/task/TraceScene';
import { TaskChatScene } from '@/pages/taskchat/TaskChatScene';
import { SettingsScene } from '@/pages/settings/SettingsScene';
import { PeriodPage } from '@/pages/placeholder/PeriodPage';

export function AppShell() {
  const location = useLocation();
  const route = useMemo(
    () => parseRoute(location.pathname + location.search + (location.hash ? '' : '')),
    [location.pathname, location.search],
  );
  const scene = sceneOf(route);
  const def = sceneDef(scene);
  // 只显示接上内核的部分（D46）：Kind 只有对话一栏；接不上内核时也只留一栏放说明。
  const soloKind = scene === 'kind';
  const panes = useMemo(
    () => (soloKind ? def.panes.filter((pane) => pane.id === 'conversation') : def.panes),
    [soloKind, def],
  );
  const dockKey = soloKind ? scene + ':solo' : scene;
  const theme = useAppStore((s) => s.theme);
  const reducedPref = useAppStore((s) => s.reducedMotion);
  const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed);
  const setSidebarCollapsed = useAppStore((s) => s.setSidebarCollapsed);
  const systemReduced = useSystemReducedMotion();
  const reduced = reducedPref ?? systemReduced;

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    if (reduced) document.documentElement.dataset.reducedMotion = 'true';
    else delete document.documentElement.dataset.reducedMotion;
  }, [theme, reduced]);

  // 只显示接上内核的部分：T055 起工作页/现在页也接真数据；其余未接场景回到 Kind。
  useEffect(() => {
    if (scene !== 'kind' && scene !== 'settings' && scene !== 'taskchat' && scene !== 'work' && scene !== 'now') {
      go('kind');
    }
  }, [scene]);

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

  // 面板内容自己从 store 取路由（dockview 面板不会跟着父组件重渲染）
  const renderPane = useCallback(
    (paneId: string) => {
      if (scene === 'kind') return <KindScene pane={paneId} />;
      if (scene === 'ability') return <AbilityScene />;
      if (scene === 'now') return <NowScene />;
      if (scene === 'work') return <WorkScene pane={paneId} />;
      if (scene === 'project' && route.kind === 'project') return <ProjectScene pane={paneId} />;
      if (scene === 'read') return <ReadScene pane={paneId} />;
      if (scene === 'task') return <TaskScene />;
      if (scene === 'taskchat') return <TaskChatScene />;
      if (scene === 'trace') return <TraceScene />;
      if (scene === 'empty' && route.id === 'now') return <EmptyNow />;
      if (scene === 'settings' && route.kind === 'settings') return <SettingsScene />;
      return <PeriodPage />;
    },
    [scene, route.id],
  );


  return (
    <div className="h-full w-full p-3 sm:p-5">
      <div
        // transform 让这个窗口成为 fixed 子元素的包含块：shadcn 的侧栏是 fixed 的，
        // 而原型是一个带外边距的「窗口」，侧栏要贴窗口而不是贴视口。
        className="relative mx-auto flex h-full w-full max-w-[1920px] overflow-hidden rounded-window border border-line bg-content text-ink shadow-[var(--shadow-pop)] [transform:translateZ(0)]"
        data-route={route.raw}
        data-scene-key={scene}
      >
        <SidebarProvider
          className="h-full min-h-0 w-full"
          open={!sidebarCollapsed}
          onOpenChange={(open: boolean) => setSidebarCollapsed(!open)}
          style={{ ['--sidebar-width' as string]: '220px', ['--sidebar-width-icon' as string]: '56px' }}
        >
          <Sidebar route={route} />
          <div className="flex min-w-0 flex-1 flex-col">
            <Toolbar route={route} />
            <main className="min-h-0 flex-1 overflow-hidden">
              <SceneDock
                key={dockKey}
                scene={dockKey}
                panes={panes}
                renderPane={renderPane}
                resetToken={0}
              />
            </main>
          </div>
        </SidebarProvider>
      </div>
    </div>
  );
}

let paintTimer: ReturnType<typeof setTimeout> | undefined;

function markRouteTracked(raw: string): void {
  markRouteStart(raw);
  clearTimeout(paintTimer);
  paintTimer = setTimeout(() => markRoutePainted(raw), 0);
}
