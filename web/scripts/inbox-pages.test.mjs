import assert from 'node:assert/strict';
import { foldProjectionPages, loadProjection } from '../src/pages/kind/inbox.ts';

const message = (id, text) => ({
  id,
  role: 'user',
  content: [{ type: 'text', text }],
  source: { kind: 'user', rpcId: 'uuid-' + id },
});
const insert = (seq, messages) => ({
  seq,
  type: 'agent/inbox/spliced',
  data: { target: 'next-turn', start: 0, inserted: messages },
});
const cancel = (seq, count) => ({
  seq,
  type: 'agent/inbox/spliced',
  data: { target: 'next-turn', start: 0, removedCount: count, outcome: 'canceled', inserted: [] },
});

// B3 单测：被清的消息插在更早一页、取消落在首屏这页。首屏那页单独折，
// 删除范围超出投影里有的消息 → 缺 2 条、不产生放回。
const firstPage = { events: [cancel(20, 2)], hasMore: true };
const alone = foldProjectionPages([firstPage.events], firstPage.events[0].seq);
assert.equal(alone.missing, 2, '首屏页单独折时，缺两条被删消息');
assert.equal(alone.restores.length, 0, '对不上时不产生放回（不静默丢也不乱放）');

// 用同一个 beforeSeq 读口往前读一页，折进投影再对：对上，放回原文。
const olderPage = {
  events: [insert(10, [message('a', '甲'), message('b', '乙')])],
  hasMore: false,
};
const fetched = [];
const loaded = await loadProjection(firstPage, async (beforeSeq) => {
  fetched.push(beforeSeq);
  return olderPage;
});
assert.deepEqual(fetched, [20], '用首屏最老 seq 当 beforeSeq 往前读');
assert.equal(loaded.missing, 0, '补上更早页后对得上');
assert.equal(loaded.restores.length, 1, '一条取消对应一次放回');
assert.equal(loaded.restores[0].seq, 20, '放回的 seq 是取消那条 splice');
assert.equal(loaded.restores[0].text, '甲' + String.fromCharCode(10) + '乙', '两条原文按顺序接起来');
assert.equal(loaded.projection.nextTurn.length, 0, '取消后队列为空');

// 读到日志开头还对不上：missing 保留、仍不放回（页面据此写「有一句排队的话没找回来」）。
const missingFirst = { events: [cancel(30, 3)], hasMore: false };
const missing = await loadProjection(missingFirst, async () => ({ events: [], hasMore: false }));
assert.equal(missing.missing, 3, '读到开头还对不上：缺 3');
assert.equal(missing.restores.length, 0, '对不上不放回');

// requireFromSeq 之前的旧取消不重放：只补队列、不产生放回。
const olderCancel = {
  events: [{ seq: 5, type: 'agent/inbox/spliced', data: { target: 'next-turn', start: 0, inserted: [message('c', '旧')] } }, { seq: 6, type: 'agent/inbox/spliced', data: { target: 'next-turn', start: 0, removedCount: 1, outcome: 'canceled', inserted: [] } }],
  hasMore: false,
};
const scoped = foldProjectionPages([olderCancel.events, [cancel(40, 0)]], 40);
assert.equal(scoped.restores.length, 0, '更早页的取消不重放');
console.log('inbox-pages ok');
