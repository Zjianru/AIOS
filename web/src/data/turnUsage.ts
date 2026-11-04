/**
 * T010.3：每轮用量的推导（照 DSH turn-usage.ts 与原型 7872a9b 的文字）。
 *
 * 一轮的用量 = 这一轮全部 assistant/message、assistant/attempt 的 usage 相加；
 * 缓存读/写只要有一次没报，这一轮就不算这一项（不显示 0% 的假事实）。
 */

export interface UsageTotals {
  input: number;
  output: number;
  cacheRead?: number;
  cacheWrite?: number;
  /** 这一轮里有一次没报缓存读（之后即使再报也不算）。 */
  cacheReadMissing?: boolean;
  cacheWriteMissing?: boolean;
}

export function emptyUsage(): UsageTotals {
  return { input: 0, output: 0 };
}

function numberField(usage: Record<string, unknown>, name: string): number | undefined {
  const value = usage[name];
  return typeof value === 'number' ? value : undefined;
}

/** 把一次调用的 usage 并进一轮的合计。 */
export function mergeUsage(totals: UsageTotals, usage: unknown): UsageTotals {
  if (!usage || typeof usage !== 'object' || Array.isArray(usage)) return totals;
  const record = usage as Record<string, unknown>;
  const next: UsageTotals = { ...totals };
  next.input += numberField(record, 'inputTokens') ?? 0;
  next.output += numberField(record, 'outputTokens') ?? 0;
  const cacheRead = numberField(record, 'cacheReadTokens');
  if (cacheRead === undefined) {
    next.cacheRead = undefined;
    next.cacheReadMissing = true;
  } else if (!next.cacheReadMissing) {
    next.cacheRead = (next.cacheRead ?? 0) + cacheRead;
  }
  const cacheWrite = numberField(record, 'cacheWriteTokens');
  if (cacheWrite === undefined) {
    next.cacheWrite = undefined;
    next.cacheWriteMissing = true;
  } else if (!next.cacheWriteMissing) {
    next.cacheWrite = (next.cacheWrite ?? 0) + cacheWrite;
  }
  return next;
}

/** 输入显示全部输入：输入 + 缓存读 + 缓存写（原型 7872a9b）。 */
export function displayedInput(totals: UsageTotals): number {
  return totals.input + (totals.cacheRead ?? 0) + (totals.cacheWrite ?? 0);
}

/** 缓存命中 = 缓存读 ÷（输入 + 缓存读 + 缓存写），四舍五入到整数百分比；没报缓存就不显示。 */
export function cacheHitPercent(totals: UsageTotals): number | null {
  if (totals.cacheRead === undefined || totals.cacheReadMissing) return null;
  const prompt = displayedInput(totals);
  if (prompt === 0) return null;
  return Math.round((totals.cacheRead / prompt) * 100);
}

/** 思考秒数：有 reasoning 才显示；不满 1 秒写 1 秒。 */
export function thinkingSeconds(reasoningMs: number): number {
  if (!(reasoningMs > 0)) return 0;
  return Math.max(1, Math.ceil(reasoningMs / 1000));
}

/** 一轮里 reasoning 帧的时间跨度：连续 run 的 dt 之和（time0 到最后一帧）。 */
export function reasoningSpanMs(stream: unknown): number {
  if (!Array.isArray(stream)) return 0;
  let span = 0;
  for (const record of stream) {
    if (!record || typeof record !== 'object') continue;
    const typed = record as { type?: string; dt?: unknown };
    if (typed.type !== 'reasoning-chunks' || !Array.isArray(typed.dt)) continue;
    for (const step of typed.dt) {
      if (typeof step === 'number') span += step;
    }
  }
  return span;
}

/** 模型显示名：在模型设置的条目里找显示名，找不到就用 id。 */
export function modelLabel(modelId: string | null | undefined, names: Record<string, string>): string {
  if (!modelId) return '模型服务';
  return names[modelId] ?? modelId;
}
