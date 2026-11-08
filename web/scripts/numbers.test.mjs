// T013.6：数字显示（清单 12 的「不到 1%」部分；原型 src/lib/numbers.ts 的口径）。
import assert from 'node:assert/strict';
import { compactPrice, formatPercent, formatCacheHitPercent } from '../src/lib/numbers.ts';

// 占用：0 就是 0%；>0 但不到 1% 写「不到 1%」；其余四舍五入。
assert.equal(formatPercent(0), '0%');
assert.equal(formatPercent(0.3), '不到 1%');
assert.equal(formatPercent(0.99), '不到 1%');
assert.equal(formatPercent(1), '1%');
assert.equal(formatPercent(1.2), '1%');
assert.equal(formatPercent(16.6), '17%');

// 缓存命中：没有输入不显示；有命中但不到 1% 也不显示成 0%；部分命中不四舍五入成 100%。
assert.equal(formatCacheHitPercent(0, 0), null);
assert.equal(formatCacheHitPercent(0, 1000), null);
assert.equal(formatCacheHitPercent(3, 1000), '不到 1%');
assert.equal(formatCacheHitPercent(200, 1250), '16%');
assert.equal(formatCacheHitPercent(996, 1000), '99.6%');
assert.equal(formatCacheHitPercent(1000, 1000), '100%');
// 模型行紧凑价：0 显示 $0（不是 $0.）；常规两位小数；小于 0.01 用两位有效数字。
assert.equal(compactPrice(0), '$0');
assert.equal(compactPrice(0.001), '$0.001');
assert.equal(compactPrice(0.004), '$0.004');
assert.equal(compactPrice(0.66), '$0.66');
assert.equal(compactPrice(1.98), '$1.98');
console.log('numbers.test.mjs: all assertions passed');
