import assert from 'node:assert/strict';
import { foldedTaskCard, reconcileTaskCards } from '../src/pages/kind/taskCardFold.ts';

// T038 折卡的纯函数用例：派发卡 + relay 的跨轮/跨页归并。
// 只给测试用到的字段（TurnViewData 的其它字段在折卡里不读）。

const dispatch = (over = {}) => ({
  id: 'task-dispatch:c1',
  taskId: 't1',
  title: '任务甲：查判重规则',
  state: 'dispatched',
  origin: 'executor',
  summary: '查一下判重',
  at: 1,
  fromDispatch: true,
  ...over,
});

const relay = (over = {}) => ({
  id: 'task-report:t1:r1',
  taskId: 't1',
  title: '任务甲：查判重规则',
  state: 'ready',
  origin: 'agent',
  summary: '查完了',
  at: 3,
  ...over,
});

const view = (id, taskResults) => ({ id, taskResults });

// 1）一条 relay（跨页：派发卡在前一页）→ 折成 1 张，位置留在派发卡那一格。
{
  const dispatchCard = dispatch();
  const relayCard = relay();
  const views = [
    view('turn-1', [dispatchCard]),
    view('turn-2', [relayCard]),
  ];
  const out = reconcileTaskCards(views);
  assert.equal(out[0].taskResults.length, 1, '派发卡位置一张');
  assert.equal(out[1].taskResults.length, 0, 'relay 卡折掉');
  assert.equal(out[0].taskResults[0].id, relayCard.id, '换成 relay 的事实（id 用 relay 的，接受决定回填用）');
  assert.equal(out[0].taskResults[0].state, 'ready');
  assert.equal(out[0].taskResults[0].fromDispatch, true, '还是派发卡的位置，标 fromDispatch 供后续辨认');
}

// 2）同一条里两条 relay 可见 → 只折第一条，第二条各留各的卡（2 张）。
{
  const second = relay({ id: 'task-report:t1:r2', state: 'blocked', summary: '又卡住了' });
  const out = reconcileTaskCards([
    view('turn-1', [dispatch()]),
    view('turn-2', [relay()]),
    view('turn-3', [second]),
  ]);
  const cards = out.flatMap((item) => item.taskResults);
  assert.equal(cards.length, 2, '折一条 + 第二条可见');
  assert.equal(cards[0].id, 'task-report:t1:r1', '第一条折回派发卡位置');
  assert.equal(cards[1].id, 'task-report:t1:r2', '第二条原样留着');
}

// 3）同轮：派发卡和第一条 relay 在同一页/同一轮 → 也只留 1 张（不按 id 误删刚换上去的新卡）。
{
  const out = reconcileTaskCards([view('turn-1', [dispatch(), relay()])]);
  assert.equal(out[0].taskResults.length, 1, '同轮也折成一张');
  assert.equal(out[0].taskResults[0].state, 'ready');
}

// 4）没有派发卡（只有 relay，旧会话/重启汇报）→ 原样不动。
{
  const onlyRelay = relay();
  const input = [view('turn-1', [onlyRelay]), view('turn-2', [relay({ id: 'task-report:t1:r2' })])];
  const out = reconcileTaskCards(input);
  assert.deepEqual(out, input, '没有派发卡：一条 relay 一张卡，不动');
}

// 5）写入范围兜底：派发时冻结的 write_scope 在 relay 没带时沿用（和同轮合并同一份策略）。
{
  const out = reconcileTaskCards([
    view('turn-1', [dispatch({ writeScope: 'hello.txt' })]),
    view('turn-2', [relay()]),
  ]);
  assert.equal(out[0].taskResults[0].writeScope, 'hello.txt');
}

// 6）foldedTaskCard 的判定：只有「派出中/进行中」的派发卡能并；已经是报告的卡不能再并。
{
  assert.ok(foldedTaskCard(dispatch(), relay()), '进行中的派发卡能并');
  assert.ok(foldedTaskCard(dispatch({ state: 'dispatching' }), relay()), '派出中也能并');
  assert.equal(foldedTaskCard(relay(), relay({ id: 'task-report:t1:r2' })), null, '已成报告的卡不再并');
}

// 7）逐轮重推：同一张 relay 在下一页又被派生一遍（id 相同）→ 重复那份丢掉，只剩主卡（T039 回归）。
{
  const foldedOnce = reconcileTaskCards([view('turn-1', [dispatch()]), view('turn-2', [relay()])]);
  // refreshTurn 重新派生 turn-2：同一张 relay 又出现在新视图里。
  const rederived = foldedOnce.map((item) =>
    item.id === 'turn-2' ? { ...item, taskResults: [relay()] } : item,
  );
  const out = reconcileTaskCards(rederived);
  const cards = out.flatMap((item) => item.taskResults);
  assert.equal(cards.length, 1, '重复的 relay 卡丢掉，只剩主卡');
  assert.equal(cards[0].id, 'task-report:t1:r1');
  assert.equal(cards[0].state, 'ready');
}

console.log('task-cards ok');
