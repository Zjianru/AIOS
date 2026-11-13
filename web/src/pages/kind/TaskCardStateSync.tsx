import { useEffect, useMemo } from 'react';

import { taskRead } from '@/data/service';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { useKindStore } from './kindStore';
import { activeTaskIds } from './taskCardState';

/** 有非终态任务卡时的轻轮询间隔（T041：最多每秒一次，实际 3 秒）。 */
const POLL_MS = 3000;

/**
 * T041：Kind 任务卡状态与任务对话同源。
 *
 * Kind 页挂载/路由进入时拉一次（用户从任务对话回来必触发），卡片集合变化（新派出/relay 终态）
 * 时再拉；还有非终态卡时每 3 秒轻轮询。页面不可见不拉；任务全到终态后轮询自动停。
 * 只拉 task-read 的头部事实（running/stopped/interrupted/reportedAt），不碰事件流。
 */
export function TaskCardStateSync() {
  const views = useKindStore((state) => state.views);
  const ids = useMemo(() => activeTaskIds(views), [views]);
  const signature = ids.join(',');

  useEffect(() => {
    if (!signature) return;
    const taskIds = signature.split(',');
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const refresh = async () => {
      const entries = await Promise.all(
        taskIds.map(async (taskId) => {
          try {
            return [taskId, (await taskRead(taskId)).task] as const;
          } catch {
            // 任务读不到（已归档/服务抖动）：保持现状，下一次轮询再试。
            return null;
          }
        }),
      );
      if (disposed) return;
      const facts = new Map<string, TaskReadHeader>();
      for (const entry of entries) {
        if (entry) facts.set(entry[0], entry[1]);
      }
      if (facts.size > 0) useKindStore.getState().setTaskCardStates(facts);
      schedule();
    };

    const schedule = () => {
      if (disposed) return;
      timer = setTimeout(() => {
        if (document.visibilityState === 'visible') void refresh();
        else schedule();
      }, POLL_MS);
    };

    const onVisible = () => {
      if (document.visibilityState === 'visible') void refresh();
    };
    document.addEventListener('visibilitychange', onVisible);
    void refresh();

    return () => {
      disposed = true;
      if (timer !== null) clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [signature]);

  return null;
}
