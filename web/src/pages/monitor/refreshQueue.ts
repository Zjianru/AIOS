/** T091.1：同一任务的连续 TaskEvent 合并成一次刷新（首帧后 delay 兑现，后帧只更新 attempt）。 */
export interface RefreshQueue {
  push: (taskId: string, attemptId: number) => void;
  clear: () => void;
}

export function createRefreshQueue(
  run: (taskId: string, attemptId: number) => void,
  delay = 350,
  schedule: (callback: () => void, ms: number) => number = (callback, ms) =>
    window.setTimeout(callback, ms),
  cancel: (handle: number) => void = (handle) => window.clearTimeout(handle),
): RefreshQueue {
  const pending = new Map<string, { timer: number; attemptId: number }>();
  return {
    push: (taskId, attemptId) => {
      const previous = pending.get(taskId);
      if (previous) {
        previous.attemptId = attemptId;
        return;
      }
      const timer = schedule(() => {
        const item = pending.get(taskId);
        pending.delete(taskId);
        run(taskId, item?.attemptId ?? attemptId);
      }, delay);
      pending.set(taskId, { timer, attemptId });
    },
    clear: () => {
      for (const item of pending.values()) cancel(item.timer);
      pending.clear();
    },
  };
}
