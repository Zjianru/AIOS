import assert from 'node:assert/strict';
import { contextMeter, formatTokens } from '../src/lib/context-meter.ts';

// 已知 used/window：百分比、色条宽度、标头读数。
const half = contextMeter(46_200, 200_000);
assert.equal(half.percent, 23, '46200/200000 四舍五入 23%');
assert.equal(half.barPercent, 23, '色条宽度跟百分比一致');
assert.equal(half.reading, '46.2k / 200.0k', '标头读数');

// 小于 1%：百分比 0，色条仍留 1% 的起点。
const tiny = contextMeter(120, 200_000);
assert.equal(tiny.percent, 0);
assert.equal(tiny.barPercent, 1);
assert.equal(tiny.reading, '120 / 200.0k');

// 封顶 99%。
const full = contextMeter(500_000, 200_000);
assert.equal(full.percent, 99);
assert.equal(full.barPercent, 99);

// window 为 0（旧服务没给）：显示 —，不编百分比。
const none = contextMeter(0, 0);
assert.equal(none.percent, 0);
assert.equal(none.barPercent, 1);
assert.equal(none.reading, '—');

// 非法 used 当 0 处理，不出现 NaN。
const bad = contextMeter(Number.NaN, 100_000);
assert.equal(bad.percent, 0);
assert.equal(bad.reading, '0 / 100.0k');

assert.equal(formatTokens(999), '999');
assert.equal(formatTokens(1000), '1.0k');
assert.equal(formatTokens(1_500_000), '1500.0k');

console.log('context-meter: all assertions passed');
