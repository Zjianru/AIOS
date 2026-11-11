// T031：一轮的轮次派生（turnViews）——同轮多张结果卡。
// 只喂日志事件，不装页面 store：派生是纯函数，Kind 与任务对话共用这一份。
import assert from 'node:assert/strict';
import { deriveTurnViews, processStepsOf } from '../src/pages/kind/turnViews.ts';

let seq = 0;
function event(type, data, time = 1794300000000 + seq) {
  seq += 1;
  return { type, seq, time, data: { turn: 1, ...data } };
}

function relayEvent(taskId, title) {
  return event('user/message', {
    message: {
      id: 'msg-relay-' + taskId,
      role: 'user',
      source: { kind: 'user', rpcId: 'task-report:' + taskId + ':report-1' },
      content: [
        {
          type: 'text',
          text: JSON.stringify({
            kind: 'task_report',
            task: { id: taskId, title, stage: 'ready_for_review' },
            origin: 'agent',
            state: 'ready',
            summary: title + '：结果在这里。',
          }),
        },
      ],
    },
  });
}

function userEvent(text) {
  return event('user/message', {
    message: {
      id: 'msg-user-1',
      role: 'user',
      source: { kind: 'user', rpcId: 'rpc-user-1' },
      content: [{ type: 'text', text }],
    },
  });
}

function assistantEvent(text) {
  return event('assistant/message', {
    message: {
      id: 'msg-assistant-' + seq,
      role: 'assistant',
      source: { kind: 'model', provider: 'deepseek', model: 'deepseek-flash' },
      content: [{ type: 'text', text }],
    },
    stream: [
      { type: 'chunk', chunk: { type: 'block-start', index: 0, blockType: 'text' } },
      { type: 'text-chunks', time0: 1, index: 0, dt: [], texts: [text] },
    ],
    usage: { inputTokens: 10, outputTokens: 4 },
  });
}

// —— 故事 1：同一轮里两条 relay → 两张卡，先到的在前 ——
{
  seq = 0;
  const views = deriveTurnViews([
    event('turn/start', {}),
    userEvent('帮我派两个任务'),
    relayEvent('task-a', '任务甲：查判重规则'),
    relayEvent('task-b', '任务乙：查归档范围'),
    assistantEvent('两张卡都在上面，具体看卡片。'),
    event('turn/end', { reason: { kind: 'completed' } }),
  ]);
  assert.equal(views.length, 1);
  assert.equal(views[0].user.text, '帮我派两个任务', '同轮用户消息照旧是气泡');
  assert.deepEqual(
    views[0].taskResults.map((fact) => fact.taskId),
    ['task-a', 'task-b'],
    '两条 relay 按日志到达顺序各出一张卡，不互相覆盖',
  );
  assert.deepEqual(
    views[0].taskResults.map((fact) => fact.title),
    ['任务甲：查判重规则', '任务乙：查归档范围'],
  );
}

// —— T024 K2：同轮用户消息 + relay，气泡和卡都在（DOM 先后由 e2e 断言） ——
{
  seq = 0;
  const views = deriveTurnViews([
    event('turn/start', {}),
    userEvent('帮我查一下'),
    relayEvent('task-a', '任务甲：查判重规则'),
    assistantEvent('结果看卡片。'),
    event('turn/end', { reason: { kind: 'completed' } }),
  ]);
  assert.equal(views[0].user.text, '帮我查一下');
  assert.equal(views[0].taskResults.length, 1);
  assert.equal(views[0].taskResults[0].taskId, 'task-a');
}

// 没 relay 的一轮：列表空，不误报。
{
  seq = 0;
  const views = deriveTurnViews([
    event('turn/start', {}),
    userEvent('你好'),
    assistantEvent('你好呀。'),
    event('turn/end', { reason: { kind: 'completed' } }),
  ]);
  assert.deepEqual(views[0].taskResults, []);
  assert.deepEqual(views[0].says, []);
  assert.equal(processStepsOf(views[0]).length, 0);
}

console.log('turn-views.test.mjs: all assertions passed');
