// T095.5：回退前端的纯函数（原文还原顺序、菜单禁用条件）。
import assert from 'node:assert/strict';
import { hasBefore, rewindSourceOfMessage, viewOfSeq } from '../src/pages/kind/rewind.ts';

const source = rewindSourceOfMessage({
  content: [
    { type: 'text', text: '打的字' },
    { type: 'text', text: '第一段粘贴', paste: true },
    { type: 'file', attachmentId: 'att-1', name: 'pic.png', bytes: 5 },
    { type: 'result-card', taskId: 'task-a', title: '结果甲', version: 2 },
    { type: 'text', text: '（来自画布「画布甲」）', canvas: { id: 'cv-a', title: '画布甲' } },
  ],
});
assert.equal(source.text, '打的字');
assert.deepEqual(source.pastes, ['第一段粘贴']);
assert.equal(source.files.length, 1);
assert.equal(source.files[0].mime, 'image/png', '旧日志没有 mime：按扩展名兜底');
assert.deepEqual(source.resultCards, [{ taskId: 'task-a', title: '结果甲', version: 2 }]);
assert.deepEqual(source.canvases, [{ id: 'cv-a', title: '画布甲' }]);
assert.equal(rewindSourceOfMessage({ content: [] }), null, '没有可还原的块时返回 null');

const rows = [
  { seq: 10, time: 1, text: '第一句' },
  { seq: 20, time: 2, text: '第二句' },
];
assert.equal(hasBefore(rows, 10), false, '压缩后第一句前面没有内容');
assert.equal(hasBefore(rows, 20), true);
assert.equal(hasBefore(rows, null), false);
assert.equal(viewOfSeq([{ id: 'turn-1', user: { seq: 10 } }], 10)?.id, 'turn-1');
assert.equal(viewOfSeq([], 10), undefined);
console.log('rewind.test.mjs: all assertions passed');
