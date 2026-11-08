/**
 * 屏蔽的模型（照 Magpie internal/provider/disabled.go，和 service 的匹配逐字一致）。
 *
 * 只用于页面显示：选择器里灰掉/标「已屏蔽」、设置页模型行上的标记。真正的拦截在服务端
 * 出站之前；「谁在用」「现在匹配几个」也由服务端给，页面不拿这份匹配当事实。
 */

/** 一个模型认哪些名字：「连接/模型」、模型 ID、ID 里最后一段（照 `namesOf`）。 */
export function namesOf(connectionId: string, modelId: string): string[] {
  const model = modelId.trim();
  const out = [model];
  if (connectionId) out.push(connectionId + '/' + model);
  const i = model.lastIndexOf('/');
  if (i >= 0 && i + 1 < model.length) out.push(model.slice(i + 1));
  return out;
}

/** 简单通配（照 `globMatch`）：* 是任意一串（可以跨 /），? 是任意一个，别的照字面。 */
export function globMatch(pat: string, s: string): boolean {
  if (!/[*?]/.test(pat)) return pat === s;
  let star = -1;
  let mark = 0;
  let pi = 0;
  let si = 0;
  while (si < s.length) {
    if (pi < pat.length && pat[pi] === '*') {
      star = pi;
      mark = si;
      pi++;
    } else if (pi < pat.length && (pat[pi] === '?' || pat[pi] === s[si])) {
      pi++;
      si++;
    } else if (star >= 0) {
      mark++;
      pi = star + 1;
      si = mark;
    } else return false;
  }
  while (pi < pat.length && pat[pi] === '*') pi++;
  return pi === pat.length;
}

/** 屏蔽了这个模型的那一条（照 `matchDisabled`）：不分大小写；没有就是 undefined。 */
export function matchDisabled(list: string[], connectionId: string, modelId: string): string | undefined {
  const names = namesOf(connectionId, modelId).map((name) => name.toLowerCase());
  for (const pattern of list) {
    const lowered = pattern.trim().toLowerCase();
    if (!lowered) continue;
    if (names.some((name) => name && globMatch(lowered, name))) return pattern;
  }
  return undefined;
}
