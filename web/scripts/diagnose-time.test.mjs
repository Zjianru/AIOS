import assert from 'node:assert/strict';
import { elapsedSeconds } from '../src/pages/monitor/diagnoseTime.ts';

// 已结束：用完 endedAt - startedAt，不随 now/tick 走。
assert.equal(elapsedSeconds(false, 1_000, 61_000, 9_999_999, 99), 60);
// 在跑：now - startedAt + tick。
assert.equal(elapsedSeconds(true, 1_000, null, 6_000, 2), 7);
// 没有 startedAt：没采到。
assert.equal(elapsedSeconds(true, null, null, 6_000, 2), null);
