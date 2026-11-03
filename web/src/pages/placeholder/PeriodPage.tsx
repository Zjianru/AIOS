/**
 * 第 2 / 3 期的占位页（需求 §三.2）：写明「第 2 期」或「第 3 期」，路由与 data-go 都通。
 */

import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { crumbOf } from '@/app/route';
import { PERIOD_OF } from '@/app/scenes';
import { Button } from '@/components/ui/button';

export function PeriodPage() {
  const route = useRouteStore((state) => state.route);
  const period = PERIOD_OF[route.kind] ?? 3;
  const title = crumbOf(route);
  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex min-h-full w-full max-w-[720px] flex-col justify-center px-8 py-14">
        <div className="brand-type text-2 tracking-[0.14em] text-ink-3">第 {period} 期</div>
        <h1 className="mt-2 text-5 font-medium text-ink">{title}</h1>
        <p className="mt-3 max-w-[560px] text-4 leading-6 text-ink-2">
          这一页排在 <b className="text-ink">第 {period} 期</b>。第 1 期只做外壳、Kind 场景、流程编辑器和测速；
          需求规定不许跨期提前做，所以这里先留一个占位页，路由、面包屑和侧栏高亮都是通的。
        </p>
        <div className="mt-6 flex flex-wrap items-center gap-2">
          <Button size="sm" className="h-8" onClick={() => go('kind')}>
            去 Kind 场景
          </Button>
          <Button variant="outline" size="sm" className="h-8" onClick={() => go('ability')}>
            去能力 → 流程
          </Button>
          <Button variant="ghost" size="sm" className="h-8 text-ink-2" onClick={() => go('now')}>
            回「现在」
          </Button>
        </div>
        <dl className="mt-10 grid grid-cols-[88px_1fr] gap-y-2 border-t border-line pt-5 text-3">
          <dt className="text-ink-3">路由</dt>
          <dd className="brand-type text-ink">#{route.raw}</dd>
          <dt className="text-ink-3">场景</dt>
          <dd className="text-ink">{route.kind}</dd>
          <dt className="text-ink-3">依据</dt>
          <dd className="text-ink-2">ROUTES.md 的期数表</dd>
        </dl>
      </div>
    </div>
  );
}
