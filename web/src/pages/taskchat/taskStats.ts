/**
 * T043：任务对话的统计只算「当前任务、当前 Attempt」的会话事件。
 *
 * task-read 给的事件就是任务最新 Attempt 的会话日志（服务端按 attempt 的 conversation 读），
 * 所以这里只按事件数轮/步、按 assistant 消息的 usage 聚合 token；绝不读 Kind 根会话的全局 stats。
 */
import type { ServiceEvent } from '@/data/service';
import { emptyUsage, mergeUsage, type UsageTotals } from '@/data/turnUsage';
import type { ModelSettingsView, ModelRow } from '@/data/keyring';

export interface TaskAttemptStats {
  /** 这个 Attempt 里起了几轮（turn/start）。 */
  turns: number;
  /** 模型回了几次（step/start；调工具会多几步）。 */
  steps: number;
  usage: UsageTotals;
}

export function taskAttemptStatsOf(events: ServiceEvent[]): TaskAttemptStats {
  let turns = 0;
  let steps = 0;
  let usage = emptyUsage();
  for (const event of events) {
    if (event.type === 'turn/start') turns += 1;
    else if (event.type === 'step/start') steps += 1;
    else if (event.type === 'assistant/message' || event.type === 'assistant/attempt') {
      const record = event.data as { usage?: unknown } | undefined;
      usage = mergeUsage(usage, record?.usage);
    }
  }
  return { turns, steps, usage };
}

/** 最近一次模型调用的输入量（输入 + 缓存读写）：上下文占用的近似「已用」。 */
export function lastPromptUsed(events: ServiceEvent[]): number | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event.type !== 'assistant/message' && event.type !== 'assistant/attempt') continue;
    const usage = (event.data as { usage?: Record<string, unknown> } | undefined)?.usage;
    if (!usage) continue;
    const number = (name: string): number => {
      const value = usage[name];
      return typeof value === 'number' ? value : 0;
    };
    const total = number('inputTokens') + number('cacheReadTokens') + number('cacheWriteTokens');
    return total > 0 ? total : null;
  }
  return null;
}

export interface TaskModelFacts {
  /** 预估花费（美元）；null = 匹配不到可靠价格，界面隐藏。 */
  cost: number | null;
  /** 上下文窗口；null = 匹配不到可靠窗口，界面隐藏。 */
  contextWindow: number | null;
  /** 最近一次调用的输入量（圆环的「已用」）。 */
  contextUsed: number | null;
}

const NO_FACTS: TaskModelFacts = { cost: null, contextWindow: null, contextUsed: null };

/**
 * 任务头只给模型显示名；从设置里的模型目录按「名字或 id 完全相等」匹配。
 * 匹配不到、或匹配到多行且价格/窗口互相矛盾，就返回 null 项（不猜、不显示错数）。
 */
export function modelFactsOf(
  model: string,
  settings: ModelSettingsView | null,
  usage: UsageTotals,
  contextUsed: number | null,
): TaskModelFacts {
  if (!settings || !model) return NO_FACTS;
  const rows: ModelRow[] = [];
  for (const connection of settings.connections) {
    rows.push(...connection.models);
    rows.push(...(connection.availableModels ?? []));
  }
  const matched = rows.filter((row) => row.name === model || row.id === model);
  if (matched.length === 0) return NO_FACTS;

  const costs = matched.map((row) => costOf(row, usage));
  const windows = matched
    .map((row) => row.contextWindow)
    .filter((value): value is number => typeof value === 'number' && value > 0);
  const uniqueCost = new Set(costs.map((value) => (value === null ? 'n' : value.toFixed(6))));
  const uniqueWindow = new Set(windows);
  return {
    cost: uniqueCost.size === 1 ? costs[0] ?? null : null,
    contextWindow: uniqueWindow.size === 1 ? windows[0] ?? null : null,
    contextUsed,
  };
}

/** 按价格表算这一次 Attempt 的花费；要用的 token 有价格缺失就算不出（返回 null）。 */
function costOf(row: ModelRow, usage: UsageTotals): number | null {
  const input = row.priceInput;
  const output = row.priceOutput;
  if (typeof input !== 'number' || typeof output !== 'number') return null;
  let cost = (usage.input * input + usage.output * output) / 1_000_000;
  if ((usage.cacheRead ?? 0) > 0) {
    const price = row.priceCacheRead;
    if (typeof price !== 'number') return null;
    cost += ((usage.cacheRead ?? 0) * price) / 1_000_000;
  }
  if ((usage.cacheWrite ?? 0) > 0) {
    const price = row.priceCacheWrite;
    if (typeof price !== 'number') return null;
    cost += ((usage.cacheWrite ?? 0) * price) / 1_000_000;
  }
  return cost;
}
