import assert from 'node:assert/strict';
import { usageResumeLine } from '../src/data/usageWait.ts';

const today = new Date(2026, 10, 25, 20, 0, 0).getTime();
assert.equal(
  usageResumeLine(new Date(2026, 10, 25, 21, 5, 0).getTime(), today),
  '用量到上限，21:05 自动接着做',
  '今天只写时刻',
);
assert.equal(
  usageResumeLine(new Date(2026, 10, 26, 9, 0, 0).getTime(), today),
  '用量到上限，11月26日 09:00 自动接着做',
  '跨天带日期',
);
assert.equal(usageResumeLine(null, today), '', '没有恢复时间不出现');
assert.equal(usageResumeLine(undefined, today), '', '没有恢复时间不出现');
console.log('usage-wait ok');
