/**
 * 界面状态（zustand）：主题、减少动效、侧栏、每个场景的 dock 布局。
 * 布局按场景存（layout-motion-v1.md §一）；窄窗收起是窗口适配，不写进这里。
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type Theme = 'dark' | 'light';

export interface AppState {
  theme: Theme;
  /** 用户手动改过的「减少动效」；null = 跟随系统。 */
  reducedMotion: boolean | null;
  sidebarCollapsed: boolean;
  /** dockview 序列化布局，按场景 key 存。 */
  layouts: Record<string, unknown>;
  setTheme: (theme: Theme) => void;
  toggleTheme: () => void;
  setReducedMotion: (value: boolean | null) => void;
  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  saveLayout: (scene: string, layout: unknown) => void;
  resetLayout: (scene: string) => void;
}

export const useAppStore = create<AppState>()(
  persist(
    (set) => ({
      theme: 'dark',
      reducedMotion: null,
      sidebarCollapsed: false,
      layouts: {},
      setTheme: (theme) => set({ theme }),
      toggleTheme: () => set((state) => ({ theme: state.theme === 'dark' ? 'light' : 'dark' })),
      setReducedMotion: (reducedMotion) => set({ reducedMotion }),
      toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
      setSidebarCollapsed: (sidebarCollapsed) => set({ sidebarCollapsed }),
      saveLayout: (scene, layout) =>
        set((state) => ({ layouts: { ...state.layouts, [scene]: layout } })),
      resetLayout: (scene) =>
        set((state) => {
          const layouts = { ...state.layouts };
          delete layouts[scene];
          return { layouts };
        }),
    }),
    {
      name: 'aios-v14',
      partialize: (state) => ({
        theme: state.theme,
        reducedMotion: state.reducedMotion,
        sidebarCollapsed: state.sidebarCollapsed,
        layouts: state.layouts,
      }),
    },
  ),
);
