/**
 * 测速用的 performance mark 收口。
 * perf.mjs 只认这里定义的名字：
 *   route:start:<route>      开始切到某个路由
 *   route:painted:<route>    该路由的首屏画完（数据到位 + 两帧 rAF 之后）
 *   turns:loaded             2000 轮对话最后一页到位
 *   turns:painted            最后一页画完
 *   prepend:start / prepend:painted   往上翻一页
 *   stream:start / stream:end         流式回复
 */

export function mark(name: string): void {
  if (typeof performance === 'undefined') return;
  performance.mark(name);
}

export function markRouteStart(route: string): void {
  mark('route:start:' + route);
}

export function markRoutePainted(route: string): void {
  requestAnimationFrame(() => {
    requestAnimationFrame(() => mark('route:painted:' + route));
  });
}

/** 取两个 mark 之间的毫秒数；缺任何一个返回 null。 */
export function durationOf(from: string, to: string): number | null {
  const a = performance.getEntriesByName(from, 'mark').at(-1);
  const b = performance.getEntriesByName(to, 'mark').at(-1);
  if (!a || !b) return null;
  return b.startTime - a.startTime;
}

/** 清掉旧的 mark，避免同一页跑多次时取错。 */
export function clearMarks(prefix?: string): void {
  performance.getEntriesByType('mark').forEach((entry) => {
    if (!prefix || entry.name.startsWith(prefix)) performance.clearMarks(entry.name);
  });
}