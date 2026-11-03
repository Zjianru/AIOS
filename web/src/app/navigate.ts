/** 跳路由：统一从 here 走，好让测速的 mark 只有一个地方打。 */

import { markRouteStart } from '@/lib/perf';

export function go(route: string): void {
  markRouteStart(route);
  const next = '#' + route.replace(/^#/, '');
  if (window.location.hash !== next) window.location.hash = next;
}

export function currentRoute(): string {
  return window.location.hash.replace(/^#\/?/, '') || 'now';
}
