/**
 * 日历页共用的一份服务端事实。读 calendar-read，做 calendar-action 后立即重读。
 * 服务推 calendar/changed 或 task-changed 才重读；页面切走就断。
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import type { CalendarActionKind } from '@/data/generated/CalendarActionKind';
import type { CalendarReadResponse } from '@/data/generated/CalendarReadResponse';
import { subscribeRootFrames } from '@/data/liveRoot';
import { calendarAction, getCalendar } from '@/data/service';

interface CalendarState {
  response: CalendarReadResponse | null;
  from: number;
  to: number;
  loading: boolean;
  error: string | null;
  selected: string | null;
  refresh: (from?: number, to?: number) => Promise<void>;
  select: (id: string | null) => void;
  act: (id: string, action: CalendarActionKind) => Promise<void>;
}

export const useCalendarStore = create<CalendarState>()((set, get) => ({
  response: null,
  from: 0,
  to: 0,
  loading: true,
  error: null,
  selected: null,
  refresh: async (from, to) => {
    const start = from ?? get().from;
    const end = to ?? get().to;
    try {
      const response = await getCalendar(start || undefined, end || undefined);
      set({ response, from: start, to: end, loading: false, error: null });
    } catch (error) {
      set({
        loading: false,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },
  select: (id) => set({ selected: id }),
  act: async (id, action) => {
    await calendarAction(id, action);
    await get().refresh(get().from, get().to);
  },
}));

/** 页面打开读一次；日历或任务事实变了才重读；切走就断。 */
export function useCalendarPolling(from: number, to: number): void {
  useEffect(() => {
    void useCalendarStore.getState().refresh(from, to);
    const unsubscribe = subscribeRootFrames((frame) => {
      const calendarChanged =
        frame.type === 'event' && frame.event.type === 'calendar/changed';
      if (frame.type === 'ready' || frame.type === 'task-changed' || calendarChanged) {
        void useCalendarStore.getState().refresh(from, to);
      }
    });
    return unsubscribe;
  }, [from, to]);
}
