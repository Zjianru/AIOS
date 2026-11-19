import assert from 'node:assert/strict';
import { reconnectDelayMs } from '../src/data/reconnect.ts';

// T067.4：断线重连退避 300/600/1200/2400/4800/5000；第 0 次不排。
assert.equal(reconnectDelayMs(0), 0);
assert.deepEqual(
  [1, 2, 3, 4, 5, 6].map((attempt) => reconnectDelayMs(attempt)),
  [300, 600, 1200, 2400, 4800, 5000],
);
assert.equal(reconnectDelayMs(7), 5000);
console.log('重连退避 300/600/1200/2400/4800/5000');
