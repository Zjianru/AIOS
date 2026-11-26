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
assert.equal(loaded.restores.length, 2, 'N2：被清的两条消息逐条放回');
assert.equal(loaded.restores[0].seq, 20, '放回的 seq 是取消那条 splice');
assert.deepEqual(
  loaded.restores.map((restore) => restore.text),
  ['甲', '乙'],
  '两条原文各自一条放回，不合并',
);
assert.deepEqual(
  loaded.restores.map((restore) => restore.key),
  ['20-0', '20-1'],
  '逐条放回的身份是 seq+index',
);
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

// T095.2 F1①②：旧日志没有 mime 时按扩展名兜底；结果卡/画布逐条放回且顺序不变。
const richMessage = (id, blocks) => ({
  id,
  role: 'user',
  content: blocks,
  source: { kind: 'user', rpcId: 'uuid-' + id },
});
const rich = foldProjectionPages(
  [
    [
      insert(50, [
        richMessage('m1', [
          { type: 'text', text: '第一条' },
          { type: 'file', attachmentId: 'att-1', name: 'pic.png', bytes: 5 },
        ]),
        richMessage('m2', [
          { type: 'text', text: '第二条' },
          { type: 'result-card', taskId: 'task-a', title: '结果甲', version: 1 },
          { type: 'result-card', taskId: 'task-b', title: '结果乙', version: 2 },
          { type: 'text', text: '（来自画布「画布甲」）', canvas: { id: 'cv-a', title: '画布甲' } },
          { type: 'text', text: '（来自画布「画布乙」）', canvas: { id: 'cv-b', title: '画布乙' } },
        ]),
      ]),
      cancel(60, 2),
    ],
  ],
  60,
);
assert.equal(rich.restores.length, 2, '两条消息逐条放回');
assert.equal(rich.restores[0].text, '第一条');
assert.equal(rich.restores[0].attachments.length, 1);
assert.equal(rich.restores[0].attachments[0].mime, 'image/png', '旧日志 file 块没有 mime：按 .png 兜底成图片');
assert.equal(rich.restores[1].text, '第二条');
assert.deepEqual(
  rich.restores[1].resultCards.map((card) => card.title),
  ['结果甲', '结果乙'],
  '结果卡按日志顺序',
);
assert.deepEqual(
  rich.restores[1].canvases.map((canvas) => canvas.title),
  ['画布甲', '画布乙'],
  '画布标记按日志顺序',
);
const unknownType = foldProjectionPages(
  [[insert(70, [richMessage('m3', [{ type: 'file', attachmentId: 'att-2', name: 'noext', bytes: 3 }])]), cancel(80, 1)]],
  80,
);
assert.equal(unknownType.restores[0].attachments[0].mime, null, '认不出扩展名才留 null（Composer 退 text/plain）');
console.log('inbox-pages ok');
