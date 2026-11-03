/** T007.9：断线重连的退避（指数递增，封顶 5s）：300/600/1200/2400/4800/5000… */
export function reconnectDelayMs(attempt: number): number {
  if (attempt < 1) return 0;
  const base = 300;
  const cap = 5_000;
  const exponential = base * 2 ** (attempt - 1);
  return Math.min(cap, exponential);
}
