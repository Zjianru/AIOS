import { useEffect, useMemo } from 'react';

import { taskRead } from '@/data/service';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { useKindStore } from './kindStore';
import { subscribeRootFrames } from '@/data/liveRoot';
import { activeTaskIds } from './taskCardState';

/**
 * T041/T051.6：Kind 任务卡状态与任务对话同源。
 *
 * Kind 页挂载/路由进入时拉一次（用户从任务对话回来必触发），卡片集合变化（新派出/relay 终态）
 * 时再拉；之后只跟根 WS 的 task-changed，空闲不请求、不轮询。
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

    const refresh = async () => {
      const entries = await Promise.all(
        taskIds.map(async (taskId) => {
          try {
            return [taskId, (await taskRead(taskId)).task] as const;
          } catch {
            // 任务读不到（已归档/服务抖动）：保持现状，下一次变化再试。
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
    };

    void refresh();
    const unsubscribe = subscribeRootFrames((frame) => {
      if (frame.type === 'ready' || (frame.type === 'task-changed' && taskIds.includes(frame.taskId))) {
        void refresh();
      }
    });

    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [signature]);

  return null;
}
