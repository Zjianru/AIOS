/**
 * T055：工作页 /「现在」共用的一份 Work 快照。
 *
 * 两页读同一个 store；页面打开读一次，服务推 task-changed 才重读，自己接受以后立刻再取一次。
 * T055.2：勾选状态与回执也在这里（两页共用；勾时存下 report_id+version，Work 刷新不改勾选快照）。
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import type { TaskAcceptManyResponse } from '@/data/generated/TaskAcceptManyResponse';
import type { WorkPick } from '@/data/generated/WorkPick';
import type { WorkResponse } from '@/data/generated/WorkResponse';
import { subscribeRootFrames } from '@/data/liveRoot';
import { acceptMany, getWork } from '@/data/service';

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

/**
 * 页面挂载时读一次，之后服务推 task-changed 才重读；离开页面就断。
 *
 * 工作场景的两个 pane 各渲染一个 WorkScene 实例，都调这个 hook；用引用计数只留一个
 * 订阅，最后一个用的人离开才断，避免同页起两根 WS。
 */
let liveUsers = 0;
let unsubscribe: (() => void) | null = null;

export function useWorkPolling(): void {
  useEffect(() => {
    liveUsers += 1;
    if (liveUsers === 1) {
      void useWorkStore.getState().refresh();
      unsubscribe = subscribeRootFrames((frame) => {
        if (frame.type === 'task-changed' || frame.type === 'ready') {
          void useWorkStore.getState().refresh();
        }
      });
    }
    return () => {
      liveUsers -= 1;
      if (liveUsers === 0 && unsubscribe !== null) {
        unsubscribe();
        unsubscribe = null;
      }
    };
  }, []);
}
