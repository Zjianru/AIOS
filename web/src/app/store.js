/**
 * 界面状态（zustand）：主题、减少动效、侧栏、假数据延迟、每个场景的 dock 布局。
 * 布局按场景存（layout-motion-v1.md §一）；窄窗收起是窗口适配，不写进这里。
 */
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
export const useAppStore = create()(persist((set) => ({
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
    setDevStressFlow: (devStressFlow) => set({ devStressFlow }),
    setFirstUse: (firstUse) => set({ firstUse }),
    setConnectedOnly: (connectedOnly) => set({ connectedOnly }),
    setKernel: (kernel) => set({ kernel }),
    setKeyringBroken: (keyringBroken) => set({ keyringBroken }),
    saveLayout: (scene, layout) => set((state) => ({ layouts: { ...state.layouts, [scene]: layout } })),
    resetLayout: (scene) => set((state) => {
        const layouts = { ...state.layouts };
        delete layouts[scene];
        return { layouts };
    }),
}), {
    name: 'aios-v14',
    partialize: (state) => ({
        theme: state.theme,
        reducedMotion: state.reducedMotion,
        sidebarCollapsed: state.sidebarCollapsed,
        latency: state.latency,
        layouts: state.layouts,
    }),
}));
