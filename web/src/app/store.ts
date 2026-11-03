/**
 * 界面状态（zustand）：主题、减少动效、侧栏、假数据延迟、每个场景的 dock 布局。
 * 布局按场景存（layout-motion-v1.md §一）；窄窗收起是窗口适配，不写进这里。
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { MockLatency } from '@/data/api';

export type Theme = 'dark' | 'light';

/**
 * 内核那边的整页状态：
 * unreachable 连不上内核；token 令牌不对；lost Kind 的对话找不到了；broken 对话日志读不下去；busy 被另一个进程占着。
 */
export type KernelDemo = 'ok' | 'unreachable' | 'token' | 'lost' | 'broken' | 'busy';

export interface AppState {
  theme: Theme;
  /** 用户手动改过的「减少动效」；null = 跟随系统。 */
  reducedMotion: boolean | null;
  sidebarCollapsed: boolean;
  latency: MockLatency;
  /** 开发开关：压测流程（60 节点）只在打开时出现在流程列表里。 */
  devStressFlow: boolean;
  /** 开发开关：首次使用（空库）——Kind 显示接模型 + 起手卡片，右栏是空的。 */
  firstUse: boolean;
  /**
   * 开发开关：只显示接上内核的部分（web/ 现阶段的样子，D46）。
   * 侧栏只剩 Kind 和设置；Kind 没有右栏、卡片、引用片、批注；设置只剩「模型与连接」；不显示花费。
   */
  connectedOnly: boolean;
  /** 开发开关：内核那边的整页状态（正常时不出现）。 */
  kernel: KernelDemo;
  /** 开发开关：Keyring 的钥匙文件读不出来（设置页和对话里都要有对应的样子）。 */
  keyringBroken: boolean;
  /** dockview 序列化布局，按场景 key 存。 */
  layouts: Record<string, unknown>;
  setTheme: (theme: Theme) => void;
  toggleTheme: () => void;
  setReducedMotion: (value: boolean | null) => void;
  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  setLatency: (value: MockLatency) => void;
  setDevStressFlow: (value: boolean) => void;
  setFirstUse: (value: boolean) => void;
  setConnectedOnly: (value: boolean) => void;
  setKernel: (value: KernelDemo) => void;
  setKeyringBroken: (value: boolean) => void;
  saveLayout: (scene: string, layout: unknown) => void;
  resetLayout: (scene: string) => void;
}

export const useAppStore = create<AppState>()(
  persist(
    (set) => ({
      theme: 'dark',
      reducedMotion: null,
      sidebarCollapsed: false,
      latency: 40,
      devStressFlow: false,
      firstUse: false,
      connectedOnly: false,
      kernel: 'ok',
      keyringBroken: false,
      layouts: {},
      setTheme: (theme) => set({ theme }),
      toggleTheme: () => set((state) => ({ theme: state.theme === 'dark' ? 'light' : 'dark' })),
      setReducedMotion: (reducedMotion) => set({ reducedMotion }),
      toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
      setSidebarCollapsed: (sidebarCollapsed) => set({ sidebarCollapsed }),
      setLatency: (latency) => set({ latency }),
      setDevStressFlow: (devStressFlow: boolean) => set({ devStressFlow }),
      setFirstUse: (firstUse: boolean) => set({ firstUse }),
      setConnectedOnly: (connectedOnly: boolean) => set({ connectedOnly }),
      setKernel: (kernel: KernelDemo) => set({ kernel }),
      setKeyringBroken: (keyringBroken: boolean) => set({ keyringBroken }),
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
        latency: state.latency,
        layouts: state.layouts,
      }),
    },
  ),
);
