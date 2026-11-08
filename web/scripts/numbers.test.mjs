// T013.6：数字显示（清单 12 的「不到 1%」部分；原型 src/lib/numbers.ts 的口径）。
import assert from 'node:assert/strict';
import {
  compactPrice, dayLabelOf, formatCacheHitPercent, formatDurationMs, formatDurationSeconds,
  formatModelTokens, formatPercent, formatTokens,
} from '../src/lib/numbers.ts';

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

// T017.1：token 两套口径（通用 / 设置页模型行）与两套时长（任务账秒 / 工作过程毫秒）。
assert.equal(formatTokens(517), '517');
assert.equal(formatTokens(1250), '1.3K');
assert.equal(formatTokens(1500), '1.5K');
assert.equal(formatTokens(65536), '65.5K');
assert.equal(formatModelTokens(1250), '1K');
assert.equal(formatModelTokens(1500), '2K');
assert.equal(formatModelTokens(65536), '66K');
assert.equal(formatDurationSeconds(0), '—');
assert.equal(formatDurationSeconds(90), '2 分');
assert.equal(formatDurationSeconds(3600), '1 小时 0 分');
assert.equal(formatDurationMs(1000), '1秒');
assert.equal(formatDurationMs(61000), '1分1秒');
assert.equal(formatDurationMs(3600000), '1小时0分');

// T017.1：日期分隔用注入的 now，跨月也算昨天；更早按「几月几日 时分」。
const fixedNow = new Date(2026, 2, 1, 12, 0, 0);
const at = (date) => date.getTime() / 1000;
assert.equal(dayLabelOf(null, fixedNow), '');
assert.equal(dayLabelOf(at(new Date(2026, 2, 1, 9, 5)), fixedNow), '今天 09:05');
assert.equal(dayLabelOf(at(new Date(2026, 1, 28, 23, 59)), fixedNow), '昨天 23:59');
assert.equal(dayLabelOf(at(new Date(2026, 1, 20, 8, 7)), fixedNow), '2月20日 08:07');
console.log('numbers.test.mjs: all assertions passed');
