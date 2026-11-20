/** 监控 WS 的任务事件去重：重连从各任务 0 补，不能用一个全局 lastSeq。 */
export interface SeenTaskSeq {
  attemptId: number;
  seq: number;
}

export function acceptTaskEvent(
  seen: Map<string, SeenTaskSeq>,
  taskId: string,
  attemptId: number,
  seq: number,
): boolean {
  const key = taskId + ':' + attemptId;
  const previous = seen.get(key);
  if (previous && seq <= previous.seq) return false;
  seen.set(key, { attemptId, seq });
  return true;
}
