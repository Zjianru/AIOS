/**
 * 聚焦条目下面那根小地图条（brief §四.2 的地铁线规则用在聚焦里）。
 * 一条线画成横条：走过站用线色实心，当前站浅底 + 状态色圈，后面的站淡到 30%；
 * 站名不画（这里只有「第几站」，名字在上一行 meta 里），所以不用 SVG，DIV 定位就能画准、缩放也清晰。
 */

import type { FocusStrip } from '@/pages/kind/focusModel';
import { cn } from '@/lib/utils';

const STATE_COLOR: Record<FocusStrip['state'], string> = {
  wait: 'var(--wait)',
  fail: 'var(--fail)',
  run: 'var(--run)',
};

export function MiniLineStrip({ strip, className }: { strip: FocusStrip; className?: string }) {
  const total = Math.max(strip.total, 2);
  const step = 100 / (total - 1);
  const current = Math.min(Math.max(strip.index - 1, 0), total - 1);
  const dots = Array.from({ length: total }, (_, index) => index);

  return (
    <div className={cn('relative mt-1.5 h-2.5', className)} aria-hidden="true">
      {/* 整条线先淡淡地铺满，再把「走到当前站」那一段盖成实色 */}
      <span
        className="absolute inset-x-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full"
        style={{ backgroundColor: strip.color, opacity: 0.28 }}
      />
      <span
        className="absolute left-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full"
        style={{ backgroundColor: strip.color, width: current * step + '%' }}
      />
      {dots.map((index) => {
        const style = { left: index * step + '%' };
        if (index === current) {
          return (
            <span
              key={index}
              className="absolute top-1/2 size-[9px] -translate-x-1/2 -translate-y-1/2 rounded-full"
              style={{
                ...style,
                backgroundColor: 'var(--stn-fill)',
                boxShadow: '0 0 0 2.5px ' + STATE_COLOR[strip.state],
              }}
            />
          );
        }
        return (
          <span
            key={index}
            className="absolute top-1/2 size-[7px] -translate-x-1/2 -translate-y-1/2 rounded-full"
            style={{ ...style, backgroundColor: strip.color, opacity: index > current ? 0.3 : 1 }}
          />
        );
      })}
    </div>
  );
}
