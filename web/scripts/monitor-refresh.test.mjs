// T091.1：TaskEvent 合并刷新队列（同任务多帧只跑一次，后帧更新 attempt；不同任务各一次）。
import assert from 'node:assert/strict';
import { createRefreshQueue } from '../src/pages/monitor/refreshQueue.ts';

const callbacks = [];
let cancelled = 0;
const runs = [];
const queue = createRefreshQueue(
  (taskId, attemptId) => runs.push({ taskId, attemptId }),
  350,
  (callback) => {
    callbacks.push(callback);
    return callbacks.length;
  },
  () => {
    cancelled += 1;
  },
);
queue.push('task-a', 1);
queue.push('task-a', 2);
queue.push('task-a', 3);
assert.equal(callbacks.length, 1, '同任务多帧只排一次');
assert.deepEqual(runs, [], '350ms 前不刷新');
callbacks.shift()();
assert.deepEqual(runs, [{ taskId: 'task-a', attemptId: 3 }], '一次刷新带最新 attempt');

queue.push('task-b', 1);
queue.push('task-c', 1);
assert.equal(callbacks.length, 2, '不同任务各排一次');
callbacks.shift()();
callbacks.shift()();
assert.deepEqual(
  runs.slice(1).sort((left, right) => left.taskId.localeCompare(right.taskId)),
  [
    { taskId: 'task-b', attemptId: 1 },
    { taskId: 'task-c', attemptId: 1 },
  ],
);

// clear 后不再触发。
queue.push('task-d', 1);
queue.clear();
assert.equal(cancelled, 1, 'clear 取消了待触发回调');
assert.deepEqual(runs.length, 3, 'clear 后不刷新');
console.log('monitor-refresh.test.mjs: all assertions passed');