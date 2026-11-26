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

/** 诊断快照按 taskId+attemptId 分开存：看旧 Attempt 时新事件不能把它盖掉。 */
export function detailKeyOf(
  taskId: string,
  attemptId: number | null | undefined,
): string {
  return taskId + '@' + (attemptId === null || attemptId === undefined ? 'latest' : String(attemptId));
}

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
      set((state) => {
        const alive = new Set(snapshot.tasks.map((task) => task.taskId));
        const details: Record<string, MonitorTaskResponse> = {};
        for (const [key, value] of Object.entries(state.details)) {
          if (alive.has(key.split('@')[0] ?? '')) details[key] = value;
        }
        const detailGenerations: Record<string, number> = {};
        for (const [key, value] of Object.entries(state.detailGenerations)) {
          if (alive.has(key.split('@')[0] ?? '')) detailGenerations[key] = value;
        }
        const detailErrors: Record<string, string> = {};
        for (const [key, value] of Object.entries(state.detailErrors)) {
          if (alive.has(key.split('@')[0] ?? '')) detailErrors[key] = value;
        }
        return { snapshot, loading: false, details, detailGenerations, detailErrors };
      });
    } catch (error) {
      if (get().snapshotGeneration !== generation) return;
      set({ snapshotError: error instanceof Error ? error.message : String(error), loading: false });
    }
  },
  loadTask: async (taskId, attemptId) => {
    // 不传 attempt 时按快照里的当前 Attempt 定位，保证 LiveCard 与事件刷新读写同一份。
    const resolved =
      attemptId ?? get().snapshot?.tasks.find((task) => task.taskId === taskId)?.attemptId;
    const key = detailKeyOf(taskId, resolved);
    const generation = (get().detailGenerations[key] ?? 0) + 1;
    set((state) => ({ detailGenerations: { ...state.detailGenerations, [key]: generation } }));
    try {
      const detail = await monitorTask(taskId, resolved ?? undefined);
      if (get().detailGenerations[key] !== generation) return;
      set((state) => ({
        details: {
          ...state.details,
          [key]: detail,
          // 卡片在快照还没给出 attemptId 时按「最新」读；这里同步一份别名，避免读不到。
          [detailKeyOf(taskId, null)]: detail,
        },
      }));
    } catch (error) {
      if (get().detailGenerations[key] !== generation) return;
      set((state) => ({
        detailErrors: {
          ...state.detailErrors,
          [key]: error instanceof Error ? error.message : String(error),
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
    const wanted = new Map<string, number | undefined>();
    for (const key of Object.keys(state.details)) {
      const [taskId, attempt] = key.split('@');
      if (taskId) wanted.set(taskId, attempt && attempt !== 'latest' ? Number(attempt) : undefined);
    }
    for (const task of state.snapshot?.tasks ?? []) {
      if (task.running) wanted.set(task.taskId, task.attemptId ?? undefined);
    }
    await Promise.all(
      [...wanted.entries()].map(([taskId, attemptId]) => get().loadTask(taskId, attemptId)),
    );
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
