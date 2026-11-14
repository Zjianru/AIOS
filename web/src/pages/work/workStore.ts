/**
 * T055：工作页 /「现在」共用的一份 Work 快照。
 *
 * 两页读同一个 store；页面开着时每 2 秒取一次（照任务对话页头的轻轮询），
 * 自己接受/点了卡以后立刻再取一次；不新建推送通道。
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import type { WorkResponse } from '@/data/generated/WorkResponse';
import { getWork } from '@/data/service';

/** 轮询间隔：两页共用（实验/验收按 2 秒内变化）。 */
export const WORK_POLL_MS = 2000;

interface WorkState {
  work: WorkResponse | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}

export const useWorkStore = create<WorkState>()((set) => ({
  work: null,
  loading: true,
  error: null,
  refresh: async () => {
    try {
      const work = await getWork();
      set({ work, loading: false, error: null });
    } catch (error) {
      set({
        loading: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },
}));

/** 页面挂载时开始取 + 两秒轻轮询；离开页面就停。 */
export function useWorkPolling(): void {
  useEffect(() => {
    void useWorkStore.getState().refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void useWorkStore.getState().refresh();
      }
    }, WORK_POLL_MS);
    return () => window.clearInterval(timer);
  }, []);
}
