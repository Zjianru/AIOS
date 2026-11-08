/**
 * 余额/套餐余量的查询节奏（T016 验收修复，照 Magpie 的默认：打开页面查一次、
 * 手动刷新、缓存 + 最小间隔）。两个数值先用常量，Claude 给具体值后只改这里。
 */
import { fetchConnectionQuota, type ServiceQuota } from '@/data/service';

/** 一次成功结果在这段时间内算新鲜：再次打开编辑器直接用缓存，不再打厂商接口。 */
export const QUOTA_CACHE_TTL_MS = 5 * 60_000;
/** 同一连接两次自动查询的最小间隔；手动刷新（force）不受这条限制。 */
export const QUOTA_MIN_INTERVAL_MS = 60_000;

interface Entry {
  quota: ServiceQuota;
  /** 结果时间（页面显示「更新于」）。 */
  at: number;
  /** 最近一次请求的时间（最小间隔从这里算）。 */
  attemptedAt: number;
}

const entries = new Map<string, Entry>();
const inflight = new Map<string, Promise<ServiceQuota>>();

export function cachedQuota(id: string): Entry | undefined {
  return entries.get(id);
}

/**
 * 查一次（或复用缓存）。force 是用户点了「刷新」：不等最小间隔，立即打厂商接口。
 * 自动查询（打开编辑器）传 false，缓存新鲜或刚查过就直接用上次结果。
 */
export async function loadQuota(
  id: string,
  { force = false }: { force?: boolean } = {},
): Promise<ServiceQuota> {
  const cached = entries.get(id);
  if (cached && !force) {
    const now = Date.now();
    if (now - cached.attemptedAt < QUOTA_MIN_INTERVAL_MS || now - cached.at < QUOTA_CACHE_TTL_MS) {
      return cached.quota;
    }
  }
  const pending = inflight.get(id);
  if (pending) return pending;
  const attemptedAt = Date.now();
  const request = fetchConnectionQuota(id)
    .then((quota) => {
      entries.set(id, { quota, at: Date.now(), attemptedAt });
      return quota;
    })
    .finally(() => {
      inflight.delete(id);
    });
  inflight.set(id, request);
  return request;
}

/** 测试用：清掉进程内缓存。 */
export function resetQuotaCache(): void {
  entries.clear();
  inflight.clear();
}
