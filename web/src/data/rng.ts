/** 固定种子的随机数：同一个种子每次生成同一段对话（需求 §二「用固定种子生成，每次一样」）。 */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** 从字符串派生一个稳定种子。 */
export function hashSeed(s: string): number {
  let h = 2166136261 >>> 0;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** 给一条数据派生它自己的随机源：index → 独立、可重现。 */
export function seededAt(seed: number, index: number): () => number {
  return mulberry32((seed ^ Math.imul(index + 1, 2654435761)) >>> 0);
}

export function pick<T>(rnd: () => number, list: readonly T[]): T {
  return list[Math.floor(rnd() * list.length) % list.length];
}

export function pickN<T>(rnd: () => number, list: readonly T[], n: number): T[] {
  const out: T[] = [];
  const pool = [...list];
  for (let i = 0; i < n && pool.length; i++) out.push(pool.splice(Math.floor(rnd() * pool.length), 1)[0]);
  return out;
}

export function intBetween(rnd: () => number, min: number, max: number): number {
  return min + Math.floor(rnd() * (max - min + 1));
}
