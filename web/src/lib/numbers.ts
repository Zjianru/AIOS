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

/** 缓存命中百分比（文本，不带 %）；没有输入时为 null。部分命中最多显示到 99.99999。 */
export function formatCacheHitPercent(cacheRead: number, prompt: number): string | null {
  if (prompt === 0) return null;
  if (cacheRead >= prompt) return '100';
  const rounded = Math.round((cacheRead / prompt) * 100);
  if (rounded < 100) return String(rounded);
  for (let places = 1; places < 6; places += 1) {
    const text = ((cacheRead / prompt) * 100).toFixed(places);
    if (Number(text) < 100) return text;
  }
  return '99.99999';
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
