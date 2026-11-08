/**
 * 设置面共享的小状态（T019.E）：显示币种/汇率 + 屏蔽表。
 *
 * 设置页不再反向 import 对话页的 kindStore：两边读同一个 store，数据仍走 data/service 与
 * data/keyring。这里只放状态和刷新动作，不碰聊天/分页。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import type { Currency } from '@/lib/numbers';
import { fetchStatus, updateSettings } from '@/data/service';
import type { ServiceStatus } from '@/data/service';
import { getModelSettings } from '@/data/keyring';

/** 显示币种与汇率（服务 settings.json + status.fx；页面只显示和切换）。 */
export interface MoneyState {
  currency: Currency;
  cnyPerUsd: number;
  /** 汇率拉到的时间（unix 秒；0 = 没取到，用 7.2）。 */
  fxAt: number;
}

const DEFAULT_MONEY: MoneyState = { currency: 'usd', cnyPerUsd: 7.2, fxAt: 0 };

/** status 里的币种/汇率并进现有值；字段缺失时保留上一次。 */
function moneyOfStatus(status: ServiceStatus, previous: MoneyState): MoneyState {
  if (!status.fx && !status.currency) return previous;
  return {
    currency: (status.currency as Currency | undefined) ?? previous.currency,
    cnyPerUsd: status.fx?.rate ?? previous.cnyPerUsd,
    fxAt: status.fx?.fetchedAt ?? previous.fxAt,
  };
}

interface SettingsState {
  money: MoneyState;
  /** 屏蔽的模型规则（服务端 settings.json 的整表）；选择器按它灰掉、标「已屏蔽」。 */
  masked: string[];
  /** 重新读屏蔽表（设置变化事件或屏蔽操作以后；读不到就保留上一次的）。 */
  refreshMasks: () => Promise<void>;
  /** 重新读币种/汇率（直接打开设置页时用）。 */
  refreshMoney: () => Promise<void>;
  /** 把一次 status 的币种/汇率并进来（kindStore 的 refreshStats 也走这里，只有一处换算）。 */
  applyStatus: (status: ServiceStatus) => void;
  /** 切币种（写 settings.json）。 */
  setCurrency: (currency: Currency) => Promise<void>;
}

export const useSettingsState = create<SettingsState>()((set, get) => ({
  money: DEFAULT_MONEY,
  masked: [],
  refreshMasks: async () => {
    try {
      const view = await getModelSettings();
      set({ masked: view.disabledModels });
    } catch {
      // 读不到设置时保留上一次的屏蔽表；服务端仍按文件里的规则拦。
    }
  },
  refreshMoney: async () => {
    try {
      get().applyStatus(await fetchStatus());
    } catch {
      // 读不到状态时保留旧值（页面不显示假数据）。
    }
  },
  applyStatus: (status) => {
    set((state) => ({ money: moneyOfStatus(status, state.money) }));
  },
  setCurrency: async (currency) => {
    try {
      const saved = (await updateSettings({ currency })) as Currency;
      set((state) => ({ money: { ...state.money, currency: saved } }));
    } catch {
      toast('币种没保存', { description: '连上内核再试一次' });
    }
  },
}));
