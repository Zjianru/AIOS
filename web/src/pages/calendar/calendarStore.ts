/**
 * T064.6：日历页共用的一份服务端事实。读 calendar-read，做 calendar-action 后立即重读。
 * 不推送：两秒轻轮询；页面切走就停。
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import type { CalendarActionKind } from '@/data/generated/CalendarActionKind';
import type { CalendarReadResponse } from '@/data/generated/CalendarReadResponse';
import { calendarAction, getCalendar } from '@/data/service';

export const CALENDAR_POLL_MS = 2000;

interface CalendarState {
  response: CalendarReadResponse | null;
  weekStart: number;
  loading: boolean;
  error: string | null;
  selected: string | null;
  refresh: (weekStart?: number) => Promise<void>;
  select: (id: string | null) => void;
  act: (id: string, action: CalendarActionKind) => Promise<void>;
}

export const useCalendarStore = create<CalendarState>()((set, get) => ({
  response: null,
  weekStart: 0,
  loading: true,
  error: null,
  selected: null,
  refresh: async (weekStart) => {
    const week = weekStart ?? get().weekStart;
    try {
      const response = await getCalendar(week || undefined);
      set({ response, weekStart: week, loading: false, error: null });
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
    await get().refresh();
  },
}));

/** 页面开着时每两秒重读；切走就清掉。 */
export function useCalendarPolling(weekStart: number): void {
  useEffect(() => {
    void useCalendarStore.getState().refresh(weekStart);
    const timer = window.setInterval(() => {
      void useCalendarStore.getState().refresh(weekStart);
    }, CALENDAR_POLL_MS);
    return () => window.clearInterval(timer);
  }, [weekStart]);
}
