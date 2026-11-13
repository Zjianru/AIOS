import { useEffect, useMemo } from 'react';

import { taskRead } from '@/data/service';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { useKindStore } from './kindStore';
import { activeTaskIds } from './taskCardState';
import { startTaskCardPoll } from './taskCardPoll';

/** 有非终态任务卡时的轻轮询间隔（T041：最多每秒一次，实际 3 秒）。 */
const POLL_MS = 3000;

/**
 * T041/T051.6：Kind 任务卡状态与任务对话同源。
 *
 * Kind 页挂载/路由进入时拉一次（用户从任务对话回来必触发），卡片集合变化（新派出/relay 终态）
 * 时再拉；还有非终态卡时每 3 秒轻轮询。页面不可见不拉；任务全到终态后轮询自动停。
 * 调度细节（在途不重发、定时器不叠、卸载后回来不写不排）在 startTaskCardPoll，单测用假定时器锁住。
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

    const poll = startTaskCardPoll({
      intervalMs: POLL_MS,
      isVisible: () => document.visibilityState === 'visible',
      refresh: async () => {
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
        // 卸载后在途回来：不写 store。
        if (disposed) return;
        const facts = new Map<string, TaskReadHeader>();
        for (const entry of entries) {
          if (entry) facts.set(entry[0], entry[1]);
        }
        if (facts.size > 0) useKindStore.getState().setTaskCardStates(facts);
      },
    });

    const onVisibility = () => poll.onVisible();
    document.addEventListener('visibilitychange', onVisibility);
    poll.refreshNow();

    return () => {
      disposed = true;
      document.removeEventListener('visibilitychange', onVisibility);
      poll.dispose();
    };
  }, [signature]);

  return null;
}
