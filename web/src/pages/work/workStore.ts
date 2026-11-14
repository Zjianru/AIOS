/**
 * T055：工作页 /「现在」共用的一份 Work 快照。
 *
 * 两页读同一个 store；页面开着时每 2 秒取一次，自己接受以后立刻再取一次；不新建推送通道。
 * T055.2：勾选状态与回执也在这里（两页共用；勾时存下 report_id+version，Work 刷新不改勾选快照）。
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import type { TaskAcceptManyResponse } from '@/data/generated/TaskAcceptManyResponse';
import type { WorkPick } from '@/data/generated/WorkPick';
import type { WorkResponse } from '@/data/generated/WorkResponse';
import { acceptMany, getWork } from '@/data/service';

/** 轮询间隔：两页共用（验收按 2 秒内变化）。 */
export const WORK_POLL_MS = 2000;

interface WorkState {
  work: WorkResponse | null;
  loading: boolean;
  error: string | null;
  /** 勾选快照：task id → 勾选那一刻看到的报告与版本。 */
  selection: Record<string, WorkPick>;
  accepting: boolean;
  receipt: TaskAcceptManyResponse | null;
  refresh: () => Promise<void>;
  togglePick: (pick: WorkPick) => void;
  selectAllAccept: () => void;
  clearSelection: () => void;
  acceptSelected: () => Promise<void>;
  acceptOne: (pick: WorkPick) => Promise<void>;
}

async function acceptPicks(picks: WorkPick[], set: (partial: Partial<WorkState>) => void): Promise<void> {
  if (picks.length === 0) return;
  set({ accepting: true, error: null });
  try {
    const receipt = await acceptMany(picks);
    set({ receipt, selection: {}, accepting: false });
    // 勾选/接受后立即重取，不等两秒轮询。
    await useWorkStore.getState().refresh();
  } catch (error) {
    set({
      accepting: false,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

export const useWorkStore = create<WorkState>()((set, get) => ({
  work: null,
  loading: true,
  error: null,
  selection: {},
  accepting: false,
  receipt: null,
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
  togglePick: (pick) => {
    const current = get().selection;
    const next = { ...current };
    if (next[pick.taskId]) delete next[pick.taskId];
    else next[pick.taskId] = pick;
    set({ selection: next });
  },
  selectAllAccept: () => {
    const wait = get().work?.wait ?? [];
    const selection: Record<string, WorkPick> = {};
    for (const item of wait) {
      if (item.kind === 'accept' && item.pick) selection[item.pick.taskId] = item.pick;
    }
    set({ selection });
  },
  clearSelection: () => set({ selection: {} }),
  acceptSelected: async () => {
    const picks = Object.values(get().selection);
    await acceptPicks(picks, set);
  },
  acceptOne: async (pick) => {
    await acceptPicks([pick], set);
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
