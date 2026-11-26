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
  /** 每次 loadTask 的代号；慢的旧响应回来时不能盖掉新的。 */
  detailLoads: Record<string, number>;
  loadSnapshot: (range?: MonitorRange) => Promise<void>;
  loadTask: (taskId: string, attemptId?: number) => Promise<void>;
  setRange: (range: MonitorRange) => void;
}

let snapshotGeneration = 0;

export const useMonitorStore = create<MonitorState>()((set, get) => ({
  range: 'today',
  snapshot: null,
  snapshotError: null,
  loading: false,
  details: {},
  detailErrors: {},
  detailLoads: {},
  setRange: (range) => {
    set({ range });
    void get().loadSnapshot(range);
  },
  loadSnapshot: async (range) => {
    const wanted = range ?? get().range;
    const generation = ++snapshotGeneration;
    set({ loading: true, snapshotError: null });
    try {
      const snapshot = await monitorSnapshot(wanted, timezone());
      if (generation !== snapshotGeneration) return;
      set({ snapshot, loading: false });
    } catch (error) {
      if (generation !== snapshotGeneration) return;
      set({ snapshotError: error instanceof Error ? error.message : String(error), loading: false });
    }
  },
  loadTask: async (taskId, attemptId) => {
    const generation = (get().detailLoads[taskId] ?? 0) + 1;
    set((state) => ({ detailLoads: { ...state.detailLoads, [taskId]: generation } }));
    try {
      const detail = await monitorTask(taskId, attemptId);
      set((state) => {
        if ((state.detailLoads[taskId] ?? 0) !== generation) return {};
        return { details: { ...state.details, [taskId]: detail } };
      });
    } catch (error) {
      set((state) => {
        if ((state.detailLoads[taskId] ?? 0) !== generation) return {};
        return {
          detailErrors: {
            ...state.detailErrors,
            [taskId]: error instanceof Error ? error.message : String(error),
          },
        };
      });
    }
  },
}));

/** 连监控 WS：ready 后逐个任务收 TaskEvent；task-changed 时重读快照。 */
export function useMonitorLive(): void {
  const loadSnapshot = useMonitorStore((state) => state.loadSnapshot);
  const loadTask = useMonitorStore((state) => state.loadTask);
  useEffect(() => {
    let socket: WebSocket | null = null;
    let closed = false;
    let timer = 0;
    let snapshotTimer = 0;
    // 任务事件很多：快照合并到一小段窗口里刷新，避免每个事件打一次 /api；
    // 待办/诊断仍由 loadTask 立即更新。
    const refreshSnapshot = () => {
      window.clearTimeout(snapshotTimer);
      snapshotTimer = window.setTimeout(() => void loadSnapshot(), 350);
    };
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
    const open = () => {
      if (closed) return;
      socket = connectMonitorSession(0, (frame) => {
        if (frame.type === 'task-event') {
          if (!accept(frame.taskId, frame.attemptId, frame.event.seq)) return;
          void loadTask(frame.taskId);
          refreshSnapshot();
        } else if (frame.type === 'task-changed') {
          // task-changed 也可能只表示「事件掉帧了」，快照之外还要重读这张卡的诊断；
          // 否则待办/当前动作会停在旧值。空 taskId 是项目变更，只刷快照。
          if (frame.taskId) void loadTask(frame.taskId);
          refreshSnapshot();
        }
      }, () => {
        if (closed) return;
        window.clearTimeout(timer);
        timer = window.setTimeout(open, 1200);
      });
    };
    open();
    return () => {
      closed = true;
      window.clearTimeout(timer);
      window.clearTimeout(snapshotTimer);
      socket?.close();
    };
  }, [loadSnapshot, loadTask]);
}
