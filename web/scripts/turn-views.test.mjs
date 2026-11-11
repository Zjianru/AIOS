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

function assistantEvent(text, extraContent = [], options = {}) {
  const toolBlocks = extraContent.filter((block) => block.type === 'tool-call');
  return event('assistant/message', {
    message: {
      id: 'msg-assistant-' + seq,
      role: 'assistant',
      source: { kind: 'model', provider: 'deepseek', model: 'deepseek-flash' },
      content: [{ type: 'text', text }, ...extraContent],
    },
    stream: [
      { type: 'chunk', chunk: { type: 'block-start', index: 0, blockType: 'text' } },
      { type: 'text-chunks', time0: 1, index: 0, dt: [], texts: [text] },
      ...toolBlocks.map((block, index) => ({
        type: 'tool-call-chunks',
        time0: 1,
        index: index + 1,
        dt: [],
        id: block.id,
        name: block.name,
        args: [block.arguments],
      })),
    ],
    usage: { inputTokens: 10, outputTokens: 4 },
    ...options,
  });
}

function toolCallEvent(callId, name, args) {
  return event('tool/call', {
    call_id: callId,
    name,
    arguments: JSON.stringify(args),
  });
}

function toolResultEvent(callId, output) {
  return event('tool/result', {
    message: {
      toolCallId: callId,
      content: [{ type: 'text', text: output }],
    },
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

// —— 故事 3：工具前说明进过程，在该工具步之前，回答区没有它 ——
{
  seq = 0;
  const say1 = '我先看一下 hello.txt。';
  const say2 = '再看一下第二个文件。';
  const views = deriveTurnViews([
    event('turn/start', {}),
    userEvent('帮我看两个文件'),
    assistantEvent(say1, [
      { type: 'tool-call', id: 'call-1', name: 'read', arguments: '{"file_path":"hello.txt"}' },
    ]),
    toolCallEvent('call-1', 'read', { file_path: 'hello.txt' }),
    toolResultEvent('call-1', 'hello from fixture'),
    assistantEvent(say2, [
      { type: 'tool-call', id: 'call-2', name: 'glob', arguments: '{"pattern":"*.txt"}' },
    ]),
    toolCallEvent('call-2', 'glob', { pattern: '*.txt' }),
    toolResultEvent('call-2', 'a.txt'),
    assistantEvent('看完了，两个文件都在。'),
    event('turn/end', { reason: { kind: 'completed' } }),
  ]);
  assert.equal(views.length, 1);
  const view = views[0];
  assert.deepEqual(
    view.says.map((say) => say.text),
    [say1, say2],
    '两句工具前说明按日志先后都留下',
  );
  assert.equal(view.answer.text, '看完了，两个文件都在。', '回答区只有最终回答');
  assert.ok(!view.answer.text.includes(say1), '工具前说明不进回答区');
  const steps = processStepsOf(view);
  assert.deepEqual(
    steps.map((step) => (step.kind === 'say' ? 'say:' + step.text : step.kind + ':' + step.name)),
    ['say:' + say1, 'tool:read', 'say:' + say2, 'tool:glob'],
    '过程里 say 步按先后插在各自工具步前面',
  );
}

// —— 取消收尾：tool-call 块被摘掉、只留 interrupted 的文字，同样进过程 ——
{
  seq = 0;
  const say = '我先确认一下你系统里对卡片的定义。';
  const views = deriveTurnViews([
    event('turn/start', {}),
    userEvent('你现在都能做什么卡片'),
    assistantEvent(say, [], {
      interrupted: true,
      stream: [
        { type: 'chunk', chunk: { type: 'block-start', index: 0, blockType: 'text' } },
        { type: 'text-chunks', time0: 1, index: 0, dt: [], texts: [say] },
        {
          type: 'tool-call-chunks',
          time0: 1,
          index: 1,
          dt: [],
          id: 'call-1',
          name: 'read',
          args: ['{"file_path":"hello.txt"}'],
        },
      ],
    }),
    event('turn/end', { reason: { kind: 'aborted' } }),
  ]);
  assert.equal(views.length, 1);
  const view = views[0];
  assert.equal(view.status, 'interrupted');
  assert.deepEqual(view.says.map((item) => item.text), [say], '取消收尾的工具前说明也进过程');
  assert.equal(view.answer, null, '回答区不显示被丢掉的那句');
  assert.deepEqual(
    processStepsOf(view).map((step) => step.kind),
    ['say'],
  );
}

// —— 只有文字的中断（被屏蔽打断的半截回答）照旧留在回答区，不当成工具前说明 ——
{
  seq = 0;
  const views = deriveTurnViews([
    event('turn/start', {}),
    userEvent('写一段话'),
    assistantEvent('我只写了一半', [], {
      interrupted: true,
      stream: [
        { type: 'chunk', chunk: { type: 'block-start', index: 0, blockType: 'text' } },
        { type: 'text-chunks', time0: 1, index: 0, dt: [], texts: ['我只写了一半'] },
      ],
    }),
    event('turn/end', { reason: { kind: 'aborted' } }),
  ]);
  assert.equal(views[0].says.length, 0);
  assert.equal(views[0].answer.text, '我只写了一半');
}

console.log('turn-views.test.mjs: all assertions passed');
