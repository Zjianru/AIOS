import assert from 'node:assert/strict';
import { acceptTaskEvent } from '../src/pages/monitor/taskSeq.ts';

const seen = new Map();
// 任务 A 的 seq 很大，任务 B 从 1 开始：重连从 0 补，不能拿 A 的 lastSeq 丢 B。
assert.equal(acceptTaskEvent(seen, 'A', 1, 100), true);
assert.equal(acceptTaskEvent(seen, 'A', 1, 101), true);
assert.equal(acceptTaskEvent(seen, 'B', 1, 1), true, '两任务 seq 差大也要收 B');
assert.equal(acceptTaskEvent(seen, 'A', 1, 90), false, '旧帧按 seq 去重');
assert.equal(acceptTaskEvent(seen, 'A', 1, 101), false, '同一帧不重复');
assert.equal(acceptTaskEvent(seen, 'A', 2, 1), true, '换 Attempt 从新 seq 开始');
assert.equal(acceptTaskEvent(seen, 'B', 1, 2), true);
