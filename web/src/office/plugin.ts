/**
 * Office 插件状态（）：页面只读开关，打开/关掉都走设置›插件同一条协议。
 *
 * 插件关时不注册预览；开关经 keyring/changed 或页面重新进入后刷新。
 */

import { create } from 'zustand';
import { pluginList } from '@/data/service';

interface OfficePluginState {
  /** Office 插件是否开着；读不到/没读过按关处理。 */
  enabled: boolean;
  loaded: boolean;
  loading: boolean;
  load: () => Promise<void>;
}

export const useOfficePluginStore = create<OfficePluginState>((set, get) => ({
  enabled: false,
  loaded: false,
  loading: false,
  load: async () => {
    if (get().loading) return;
    set({ loading: true });
    try {
      const response = await pluginList();
      const office = response.plugins.find((plugin) => plugin.id === 'office');
      set({ enabled: office?.enabled ?? false, loaded: true, loading: false });
    } catch {
      set({ enabled: false, loaded: true, loading: false });
    }
  },
}));

/** 组件挂载时拉一次；已读过不重复拉。 */
export function ensureOfficePluginLoaded() {
  const state = useOfficePluginStore.getState();
  if (!state.loaded && !state.loading) void state.load();
}

/** 设置页/确认卡改变开关后刷新。 */
export function refreshOfficePlugin() {
  return useOfficePluginStore.getState().load();
}
