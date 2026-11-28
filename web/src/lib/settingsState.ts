/**
 * 设置面共享的小状态：屏蔽表。
 *
 * 设置页不再反向 import 对话页的 kindStore：两边读同一个 store，数据仍走 data/keyring。
 * 这里只放状态和刷新动作，不碰聊天/分页。
 */

import { create } from 'zustand';
import { getModelSettings } from '@/data/keyring';

interface SettingsState {
  /** 屏蔽的模型规则（服务端 settings.json 的整表）；选择器按它灰掉、标「已屏蔽」。 */
  masked: string[];
  /** 重新读屏蔽表（设置变化事件或屏蔽操作以后；读不到就保留上一次的）。 */
  refreshMasks: () => Promise<void>;
}

export const useSettingsState = create<SettingsState>()((set) => ({
  masked: [],
  refreshMasks: async () => {
    try {
      const view = await getModelSettings();
      set({ masked: view.disabledModels });
    } catch {
      // 读不到设置时保留上一次的屏蔽表；服务端仍按文件里的规则拦。
    }
  },
}));
