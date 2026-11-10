// 任务对话的 live 合并与收尾守卫（spec 用户故事 4；F1 的 waiting 后新 turn 用例）。
// 期望值是手写的日志事实（seq 单调、报告时间与轮开始时间的先后），不来自页面输出。
import assert from 'node:assert/strict';
import { mergeLiveEvent, withReportedCompletion } from '../src/pages/taskchat/taskLive.ts';

function event(seq, type = 'assistant/message') {
  return { seq, time: seq * 1000, type, data: {} };
}

assert.deepEqual(
  mergeLiveEvent([event(1), event(2)], event(2)).map((item) => item.seq),
  [1, 2],
  '重复送来的同一 seq 只留一次',
);
assert.deepEqual(
  mergeLiveEvent([event(1), event(3)], event(2)).map((item) => item.seq),
  [1, 3],
  '倒退的事件丢掉',
);
assert.deepEqual(
  mergeLiveEvent([event(1)], event(2)).map((item) => item.seq),
  [1, 2],
  '新事件追加在末尾',
);

function view(overrides = {}) {
  return {
    id: 'turn-1',
    startedAt: 1_000,
    durationMs: null,
    status: 'inProgress',
    user: null,
    taskResult: null,
    reasoning: null,
    tools: [],
    answer: null,
    thinkingSeconds: 0,
    tokens: { input: 0, output: 0 },
    model: 'deepseek-flash',
    cost: null,
    retry: null,
    failure: null,
    question: null,
    compaction: null,
    canvas: null,
    card: null,
    delivery: null,
    stoppedAt: null,
    ...overrides,
  };
}

// 终态报告后会话立刻收掉，日志没有 turn/end：报告时间在这一轮开始之后，收尾这一轮。
const headless = withReportedCompletion([view()], 1_005_000);
assert.equal(headless[0].status, 'completed');
assert.equal(headless[0].durationMs, 5_000);

// waiting 报告之后 task_status 起的新一轮：报告早于本轮开始，不能标完成。
const newTurnAfterWaiting = withReportedCompletion([view({ startedAt: 2_010 })], 2_000_000);
assert.equal(newTurnAfterWaiting[0].status, 'inProgress');
assert.equal(newTurnAfterWaiting[0].durationMs, null);

// 还没报告：页面照日志显示。
assert.equal(withReportedCompletion([view()], null)[0].status, 'inProgress');
assert.equal(withReportedCompletion([view()], undefined)[0].status, 'inProgress');

// 已有 turn/end 的一轮不用报告来收尾。
assert.equal(
  withReportedCompletion([view({ status: 'completed', durationMs: 42 })], 1_500_000)[0].durationMs,
  42,
);

// 没有轮开始时间就无从判断，宁可不收尾。
assert.equal(withReportedCompletion([view({ startedAt: null })], 1_500_000)[0].status, 'inProgress');

console.log('taskchat live checks passed');
