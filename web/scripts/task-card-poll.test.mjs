import assert from 'node:assert/strict';
import { startTaskCardPoll } from '../src/pages/kind/taskCardPoll.ts';

// T051.6：Kind 任务卡轮询不叠——单测用假定时器和可控拉取，验证：
// 1) 有拉取在途时 visibilitychange 不再发起第二次；
// 2) 排下一次之前清掉旧定时器（切走再切回后不会多出一串定时器）；
// 3) 卸载后在途的拉取回来：不写、不再排。

function fakeTimers() {
  let next = 1;
  const pending = new Map();
  return {
    setTimeout(handler, ms) {
      const id = next++;
      pending.set(id, { handler, ms });
      return id;
    },
    clearTimeout(id) {
      pending.delete(id);
    },
    count() {
      return pending.size;
    },
    async fireNext() {
      const [id, entry] = [...pending.entries()][0];
      pending.delete(id);
      entry.handler();
      await new Promise((resolve) => setImmediate(resolve));
    },
  };
}

function controlledRefresh() {
  const resolvers = [];
  let calls = 0;
  return {
    calls: () => calls,
    refresh: () => {
      calls += 1;
      return new Promise((resolve) => resolvers.push(resolve));
    },
    async resolveNext() {
      const resolve = resolvers.shift();
      resolve();
      await new Promise((done) => setImmediate(done));
    },
  };
}

// 1) 在途时 visibilitychange 不再发起第二次。
{
  const timers = fakeTimers();
  const controlled = controlledRefresh();
  const poll = startTaskCardPoll({
    intervalMs: 3000,
    refresh: controlled.refresh,
    isVisible: () => true,
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
  });
  poll.refreshNow();
  assert.equal(controlled.calls(), 1, '初始拉取一次');
  poll.onVisible();
  poll.onVisible();
  assert.equal(controlled.calls(), 1, '有拉取在途时 visibilitychange 不能再发起');
  await controlled.resolveNext();
  poll.dispose();
}

// 2) 切走再切回：回来后重新排之前清掉旧定时器，只留一个。
{
  const timers = fakeTimers();
  const controlled = controlledRefresh();
  const poll = startTaskCardPoll({
    intervalMs: 3000,
    refresh: controlled.refresh,
    isVisible: () => true,
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
  });
  poll.refreshNow();
  await controlled.resolveNext();
  assert.equal(timers.count(), 1, '一次拉取后只有一个下一次定时器');
  poll.onVisible();
  await controlled.resolveNext();
  assert.equal(timers.count(), 1, 'visibilitychange 触发的拉取回来后不能多出定时器');
  poll.dispose();
  assert.equal(timers.count(), 0, '卸载后定时器清空');
}

// 3) 卸载后在途的拉取回来：不再排下一次。
{
  const timers = fakeTimers();
  const controlled = controlledRefresh();
  const poll = startTaskCardPoll({
    intervalMs: 3000,
    refresh: controlled.refresh,
    isVisible: () => true,
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
  });
  poll.refreshNow();
  await controlled.resolveNext(); // 第一次完成，排了一个
  assert.equal(timers.count(), 1);
  await timers.fireNext(); // 定时器触发第二次拉取（在途）
  assert.equal(controlled.calls(), 2);
  poll.dispose();
  await controlled.resolveNext();
  assert.equal(timers.count(), 0, '卸载后在途回来不再排定时器');
}

console.log('taskCardPoll: ok');
