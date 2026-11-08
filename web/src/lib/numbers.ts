/**
 * 数字的显示：token 数、缓存命中、花费（照原型 app-v14 1f0c920 的 lib/numbers，数据源换成服务给的币种/汇率）。
 *
 * - token：照 DSH `ui-chat/token-format.ts`：517 / 12.2K / 517K / 1.2M；精确值按三位分组，悬停时给。
 * - 缓存命中：照 DSH `formatCacheHitPercent`：没有输入不显示；部分命中不四舍五入成 100%。
 * - 花费：照 Magpie `usage.FormatCost`：价格和花费一律按美元存，显示时按币种换算；
 *   ≥100 取整，≥1 两位小数，其余三位；有一部分调用没价格，后面加「+」。
 */

export type Currency = 'usd' | 'cny';

export const CURRENCIES: { id: Currency; label: string }[] = [
  { id: 'usd', label: '$ USD' },
  { id: 'cny', label: '¥ CNY' },
];

export function formatTokens(value: number): string {
  const scaled = (candidate: number): string =>
    candidate >= 100 ? String(Math.round(candidate)) : String(Math.round(candidate * 10) / 10);
  if (value < 1_000) return String(value);
  if (value < 1_000_000) return scaled(value / 1_000) + 'K';
  return scaled(value / 1_000_000) + 'M';
}

export function formatExactTokens(value: number): string {
  return value.toLocaleString('en-US');
}

/** 缓存命中的文本（带 %）；没有命中时为 null；有命中但不到 1% 写「不到 1%」。 */
export function formatCacheHitPercent(cacheRead: number, prompt: number): string | null {
  if (prompt === 0 || cacheRead <= 0) return null;
  if (cacheRead >= prompt) return '100%';
  const percent = (cacheRead / prompt) * 100;
  if (percent < 1) return '不到 1%';
  const rounded = Math.round(percent);
  if (rounded < 100) return rounded + '%';
  for (let places = 1; places < 6; places += 1) {
    const text = percent.toFixed(places);
    if (Number(text) < 100) return text + '%';
  }
  return '99.99999%';
}

/** 上下文占用的百分比（带 %）：有占用但不到 1% 写「不到 1%」，不显示成 0%；其余四舍五入。 */
export function formatPercent(value: number): string {
  if (value > 0 && value < 1) return '不到 1%';
  return Math.round(value) + '%';
}
/** 模型行的紧凑单价（美元/百万 token）：常规两位小数，小于 0.01 用两位有效数字；0 写 $0。 */
export function compactPrice(value: number): string {
  // 0 价就是免费：直接写 $0，别让两位有效数字留下一个悬空的 "$0."。
  if (value === 0) return '$0';
  if (value >= 0.01) return '$' + value.toFixed(2);
  // 缓存价可能小于 0.01，两小数会变成 0；用两位有效数字。
  return '$' + value.toPrecision(2).replace(/0+$/, '');
}

/** 花费（美元）按币种显示；cnyPerUsd 是汇率（没取到时 7.2）；partial：有调用没价格，后面加「+」。 */
export function formatCost(usd: number, currency: Currency, cnyPerUsd: number, partial = false): string {
  let amount = usd;
  let sign = '$';
  if (currency === 'cny' && cnyPerUsd > 0) {
    amount = usd * cnyPerUsd;
    sign = '¥';
  }
  const text = amount >= 100 ? amount.toFixed(0) : amount >= 1 ? amount.toFixed(2) : amount.toFixed(3);
  return sign + text + (partial ? '+' : '');
}

/** 汇率拉到时间的显示（页面不推算，直接用服务给的 fetchedAt 毫秒）。 */
export function formatFxTime(fetchedAt: number): string | null {
  if (!fetchedAt) return null;
  const date = new Date(fetchedAt * 1000);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}
