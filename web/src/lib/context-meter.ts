/**
 * Kind 右栏「Kind 现在带着」的读数（T098.2 F1 抽取）。
 *
 * 从 ContextUsage 的真值 used / window 算：占用百分比、色条宽度百分比、标头读数。
 * 只做展示换算，不估算分块、不补数（分块估算列后续）。
 */

export function formatTokens(tokens: number): string {
  if (tokens >= 1000) return (tokens / 1000).toFixed(1) + 'k';
  return String(tokens);
}

export interface ContextMeter {
  /** 占用百分比（0–99，和底栏 usage 同一口径；window 为 0 时 0）。 */
  percent: number;
  /** 色条填充宽度（至少 1%，让 0% 也有一条可见的起点；window 为 0 时 1）。 */
  barPercent: number;
  /** 标头读数，例如 46.2k / 200.0k；window 为 0 时「—」。 */
  reading: string;
}

export function contextMeter(used: number, window: number): ContextMeter {
  if (!Number.isFinite(window) || window <= 0) {
    return { percent: 0, barPercent: 1, reading: '—' };
  }
  const safeUsed = Number.isFinite(used) && used > 0 ? used : 0;
  const percent = Math.min(99, Math.round((safeUsed / window) * 100));
  return {
    percent,
    barPercent: Math.max(1, percent),
    reading: formatTokens(safeUsed) + ' / ' + formatTokens(window),
  };
}
