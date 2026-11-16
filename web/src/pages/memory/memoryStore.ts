/**
 * 记忆页的数据与动作：一次 MemoryRead 读全量，MemoryAction 做一次变更；
 * 服务推 memory/changed { version } 就重读（不加轮询）。
 */

import { create } from 'zustand';
import { toast } from 'sonner';
import { connectSession, memoryAction, memoryRead } from '@/data/service';
import type { MemoryActionKind, MemoryReadResponse } from '@/data/service';

interface MemoryState {
  data: MemoryReadResponse | null;
  loading: boolean;
  error: string | null;
  query: string;
  setQuery: (query: string) => void;
  load: (query?: string) => Promise<void>;
  run: (action: MemoryActionKind) => Promise<void>;
}

let socket: WebSocket | null = null;
let reconnectTimer: number | null = null;

function closeLive(): void {
  socket?.close();
  socket = null;
  if (reconnectTimer !== null) {
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

function openLive(): void {
  socket = connectSession(
    Number.MAX_SAFE_INTEGER,
    (frame) => {
      if (frame.type !== 'event') return;
      // 记忆库变了，或做梦跑完出了卡：都重读（run 的状态/明细要跟着变）。
      if (frame.event.type === 'memory/changed' || frame.event.type === 'memory/card') {
        void useMemoryState.getState().load();
      }
    },
    () => {
      socket = null;
      if (reconnectTimer === null) {
        reconnectTimer = window.setTimeout(() => {
          reconnectTimer = null;
          openLive();
        }, 1000);
      }
    },
  );
}

export const useMemoryState = create<MemoryState>((set, get) => ({
  data: null,
  loading: false,
  error: null,
  query: '',
  setQuery: (query) => set({ query }),
  load: async (query) => {
    set({ loading: true });
    try {
      const data = await memoryRead({
        query: (query ?? get().query).trim() || undefined,
      });
      set({ data, loading: false, error: null });
    } catch (error) {
      set({
        loading: false,
        error: error instanceof Error ? error.message : '读不出记忆',
      });
    }
  },
  run: async (action) => {
    try {
      const response = await memoryAction(action);
      if (response.message) toast(response.message);
      await get().load();
    } catch (error) {
      toast(error instanceof Error ? error.message : '没成功');
    }
  },
}));

/** 进页面时接上 live 推送并读一次；离开时断开。 */
export function mountMemoryLive(): () => void {
  openLive();
  void useMemoryState.getState().load();
  return () => closeLive();
}
