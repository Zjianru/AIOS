/**
 * 监控页的数据：快照/诊断全部来自服务端，只有本地 elapsed 与 WS 帧驱动重读；
 * 没有定时轮询——重连按最后事件序号补，task-changed 时重读快照。
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import {
  connectMonitorSession,
  monitorSnapshot,
  monitorTask,
} from '@/data/service';
import { createRefreshQueue } from './refreshQueue';
import type {
  MonitorRange,
  MonitorSnapshotResponse,
  MonitorTaskResponse,
} from '@/data/service';

export type MonitorTab = 'cost' | 'live';

function timezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

interface MonitorState {
  range: MonitorRange;
  snapshot: MonitorSnapshotResponse | null;
  snapshotError: string | null;
  loading: boolean;
  details: Record<string, MonitorTaskResponse>;
  detailErrors: Record<string, string>;
  /** 代次：慢回来的旧快照/旧诊断对不上就丢，不盖新值。 */
  snapshotGeneration: number;
  detailGenerations: Record<string, number>;
  loadSnapshot: (range?: MonitorRange) => Promise<void>;
  loadTask: (taskId: string, attemptId?: number) => Promise<void>;
  /** 事件驱动的合并刷新：快照 + 该任务诊断。 */
  refreshTask: (taskId: string, attemptId?: number) => Promise<void>;
  /** ready/重连后补一次：快照 + 已在看的诊断。 */
  refreshAll: () => Promise<void>;
  setRange: (range: MonitorRange) => void;
}

export const useMonitorStore = create<MonitorState>()((set, get) => ({
  range: 'today',
  snapshot: null,
  snapshotError: null,
  loading: false,
  details: {},
  detailErrors: {},
  snapshotGeneration: 0,
  detailGenerations: {},
  setRange: (range) => {
    set({ range });
    void get().loadSnapshot(range);
  },
  loadSnapshot: async (range) => {
    const wanted = range ?? get().range;
    const generation = get().snapshotGeneration + 1;
    set({ loading: true, snapshotError: null, snapshotGeneration: generation });
    try {
      const snapshot = await monitorSnapshot(wanted, timezone());
      if (get().snapshotGeneration !== generation) return;
      set({ snapshot, loading: false });
    } catch (error) {
      if (get().snapshotGeneration !== generation) return;
      set({ snapshotError: error instanceof Error ? error.message : String(error), loading: false });
    }
  },
  loadTask: async (taskId, attemptId) => {
    const generation = (get().detailGenerations[taskId] ?? 0) + 1;
    set((state) => ({ detailGenerations: { ...state.detailGenerations, [taskId]: generation } }));
    try {
      const detail = await monitorTask(taskId, attemptId);
      if (get().detailGenerations[taskId] !== generation) return;
      set((state) => ({ details: { ...state.details, [taskId]: detail } }));
    } catch (error) {
      if (get().detailGenerations[taskId] !== generation) return;
      set((state) => ({
        detailErrors: {
          ...state.detailErrors,
          [taskId]: error instanceof Error ? error.message : String(error),
        },
      }));
    }
  },
  refreshTask: async (taskId, attemptId) => {
    await Promise.all([get().loadSnapshot(), get().loadTask(taskId, attemptId)]);
  },
  refreshAll: async () => {
    await get().loadSnapshot();
    const state = get();
    const ids = new Set<string>(Object.keys(state.details));
    for (const task of state.snapshot?.tasks ?? []) if (task.running) ids.add(task.taskId);
    await Promise.all([...ids].map((taskId) => get().loadTask(taskId)));
  },
}));

/** 连监控 WS：ready 后补一次；task-event 按任务合并 350ms 后刷快照+诊断；
 * task-changed 刷快照并重读该任务诊断；慢响应由 store 按代次丢弃。 */
export function useMonitorLive(): void {
  const refreshTask = useMonitorStore((state) => state.refreshTask);
  const refreshAll = useMonitorStore((state) => state.refreshAll);
  useEffect(() => {
    let socket: WebSocket | null = null;
    let closed = false;
    let reconnectTimer = 0;
    // 重连从各任务 0 补：lastSeq 是全局的，两个任务 seq 差大时会把另一个任务的
    // 事件当成已读；这里按 task+attempt 记已收到的最大 seq，重复帧直接丢。
    const seen = new Map<string, { attemptId: number; seq: number }>();
    const accept = (taskId: string, attemptId: number, seq: number) => {
      const key = taskId + ':' + attemptId;
      const previous = seen.get(key);
      if (previous && seq <= previous.seq) return false;
      seen.set(key, { attemptId, seq });
      return true;
    };
    // 同一任务的连续事件只发一次快照+诊断；首帧后 350ms 兑现，后面的帧只更新 attempt。
    const queue = createRefreshQueue((taskId, attemptId) => {
      void refreshTask(taskId, attemptId);
    });
    const open = () => {
      if (closed) return;
      socket = connectMonitorSession(0, (frame) => {
        if (frame.type === 'ready') {
          void refreshAll();
        } else if (frame.type === 'task-event') {
          if (!accept(frame.taskId, frame.attemptId, frame.event.seq)) return;
          queue.push(frame.taskId, frame.attemptId);
        } else if (frame.type === 'task-changed') {
          // 空 taskId 是项目库变了：补快照和已在看的诊断；具体任务则两样都刷。
          if (frame.taskId) void refreshTask(frame.taskId);
          else void refreshAll();
        }
      }, () => {
        if (closed) return;
        window.clearTimeout(reconnectTimer);
        reconnectTimer = window.setTimeout(open, 1200);
      });
    };
    open();
    return () => {
      closed = true;
      window.clearTimeout(reconnectTimer);
      queue.clear();
      socket?.close();
    };
  }, [refreshAll, refreshTask]);
}
