import assert from 'node:assert/strict';
import {
  cacheHitPercent,
  contextPercent,
  contextTokens,
  displayedInput,
  emptyUsage,
  mergeUsage,
  modelLabel,
  reasoningSpanMs,
  thinkingSeconds,
} from '../src/data/turnUsage.ts';

// 清单 4：两次调用相加；一次没报缓存读 → 没有缓存命中。
let usage = emptyUsage();
usage = mergeUsage(usage, { inputTokens: 100, outputTokens: 10, cacheReadTokens: 40, cacheWriteTokens: 10 });
usage = mergeUsage(usage, { inputTokens: 200, outputTokens: 20, cacheReadTokens: 60, cacheWriteTokens: 0 });
assert.equal(usage.input, 300);
assert.equal(usage.output, 30);
assert.equal(usage.cacheRead, 100);
assert.equal(usage.cacheWrite, 10);
assert.equal(displayedInput(usage), 410);
assert.equal(cacheHitPercent(usage), Math.round((100 / 410) * 100));

let missing = emptyUsage();
missing = mergeUsage(missing, { inputTokens: 100, outputTokens: 5, cacheReadTokens: 50 });
missing = mergeUsage(missing, { inputTokens: 200, outputTokens: 5 });
assert.equal(missing.cacheRead, undefined);
assert.equal(cacheHitPercent(missing), null, '一次没报缓存读 → 这一轮不算缓存');

let zeroInput = emptyUsage();
zeroInput = mergeUsage(zeroInput, { inputTokens: 0, outputTokens: 5, cacheReadTokens: 0, cacheWriteTokens: 0 });
assert.equal(cacheHitPercent(zeroInput), null, '输入为 0 不显示命中');

// 清单 5：模型显示名。
assert.equal(modelLabel('deepseek-chat', { 'deepseek-chat': 'DeepSeek Chat' }), 'DeepSeek Chat');
assert.equal(modelLabel('deepseek-flash', {}), 'deepseek-flash');
assert.equal(modelLabel(null, {}), '模型服务');

// 清单 6：思考秒数边界 + 从 reasoning 帧算跨度。
assert.equal(thinkingSeconds(0), 0);
assert.equal(thinkingSeconds(200), 1, '不满 1 秒写 1 秒');
assert.equal(thinkingSeconds(1000), 1);
assert.equal(thinkingSeconds(1500), 2);
assert.equal(
  reasoningSpanMs([
    { type: 'reasoning-chunks', time0: 1, index: 0, dt: [200, 300], texts: ['先想', '一下'] },
    { type: 'text-chunks', time0: 1, index: 1, dt: [], texts: ['答'] },
  ]),
  500,
);
// spec：用 time0 和 dt 算跨度；两段 reasoning 中间隔 5 秒，间隔也要算进去。
assert.equal(
  reasoningSpanMs([
    { type: 'reasoning-chunks', time0: 1_000, index: 0, dt: [200], texts: ['先'] },
    { type: 'reasoning-chunks', time0: 7_000, index: 0, dt: [], texts: ['后'] },
  ]),
  6_000,
);

// 清单 7：压缩占用的前后 context 与百分比（窗口未知/0 不显示）。
assert.equal(contextTokens({ inputTokens: 100, cacheReadTokens: 20, cacheWriteTokens: 5 }), 125);
assert.equal(contextPercent(125, 500), 25);
assert.equal(contextPercent(0, 500), 0);
assert.equal(contextPercent(4500, 1_000_000), 0);
assert.equal(contextPercent(125, undefined), null, '窗口拿不到就不显示占用');
assert.equal(contextPercent(125, 0), null);

console.log('turn-usage 单测通过（清单 4/5/6/7 的数据层部分）');
