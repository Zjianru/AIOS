// T013.6：数字显示（清单 12 的「不到 1%」部分；原型 src/lib/numbers.ts 的口径）。
import assert from 'node:assert/strict';
import { formatPercent, formatCacheHitPercent } from '../src/lib/numbers.ts';

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
console.log('numbers.test.mjs: all assertions passed');
