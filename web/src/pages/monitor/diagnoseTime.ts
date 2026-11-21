/** 诊断页「用时」：在跑才把本地 tick 加上去，结束用 endedAt 静态算。 */
export function elapsedSeconds(
  running: boolean,
  startedAt: number | null | undefined,
  endedAt: number | null | undefined,
  nowMs: number,
  tick: number,
): number | null {
  if (startedAt == null) return null;
  if (running) return Math.max(0, (nowMs - startedAt) / 1000) + tick;
  if (endedAt == null) return null;
  return Math.max(0, (endedAt - startedAt) / 1000);
}
