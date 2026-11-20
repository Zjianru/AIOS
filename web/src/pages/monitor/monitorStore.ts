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
  loadSnapshot: (range?: MonitorRange) => Promise<void>;
  loadTask: (taskId: string, attemptId?: number) => Promise<void>;
  setRange: (range: MonitorRange) => void;
}

export const useMonitorStore = create<MonitorState>()((set, get) => ({
  range: 'today',
  snapshot: null,
  snapshotError: null,
  loading: false,
  details: {},
  detailErrors: {},
  setRange: (range) => {
    set({ range });
    void get().loadSnapshot(range);
  },
  loadSnapshot: async (range) => {
    const wanted = range ?? get().range;
    set({ loading: true, snapshotError: null });
    try {
      const snapshot = await monitorSnapshot(wanted, timezone());
      set({ snapshot, loading: false });
    } catch (error) {
      set({ snapshotError: error instanceof Error ? error.message : String(error), loading: false });
    }
  },
  loadTask: async (taskId, attemptId) => {
    try {
      const detail = await monitorTask(taskId, attemptId);
      set((state) => ({ details: { ...state.details, [taskId]: detail } }));
    } catch (error) {
      set((state) => ({
        detailErrors: {
          ...state.detailErrors,
          [taskId]: error instanceof Error ? error.message : String(error),
        },
      }));
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
    let lastSeq = 0;
    const open = () => {
      if (closed) return;
      socket = connectMonitorSession(lastSeq, (frame) => {
        if (frame.type === 'task-event') {
          lastSeq = Math.max(lastSeq, frame.event.seq);
          void loadTask(frame.taskId);
        } else if (frame.type === 'task-changed') {
          void loadSnapshot();
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
      socket?.close();
    };
  }, [loadSnapshot, loadTask]);
}
