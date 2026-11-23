import assert from 'node:assert/strict';
import {
  kindEmojiChar,
  partialKindReactId,
  partialKindSayParts,
} from '../src/pages/kind/kindOutput.ts';


assert.equal(kindEmojiChar('heart'), '❤️');
assert.deepEqual(partialKindSayParts('{"parts":["第一段","第二'), ['第一段']);
assert.deepEqual(partialKindSayParts('{"parts":["第一段","第二段"]}'), ['第一段', '第二段']);
assert.equal(partialKindReactId('{"emoji_id":"+1"}'), '+1');

console.log('kind output: partial say/react parsing ok');

const { deriveTurnViews, processStepsOf } = await import('../src/pages/kind/turnViews.ts');
const events = [
  { seq: 1, type: 'turn/start', time: 1000, data: { turn: 1 } },
  { seq: 2, type: 'user/message', time: 1001, data: { turn: 1, message: { role: 'user', source: { kind: 'user', rpcId: 'u1' }, content: [{ type: 'text', text: '谢谢' }] } } },
  { seq: 3, type: 'tool/call', time: 1002, data: { turn: 1, step: 1, callId: 'r1', name: 'kind_react', arguments: '{"emoji_id":"heart"}' } },
  { seq: 4, type: 'tool/result', time: 1003, data: { turn: 1, step: 1, message: { role: 'tool', toolCallId: 'r1', content: 'ok' }, meta: { kind_react: { emoji_id: 'heart' } } } },
  { seq: 5, type: 'tool/call', time: 1004, data: { turn: 1, step: 1, callId: 's1', name: 'kind_say', arguments: '{"parts":["甲","乙"]}' } },
  { seq: 6, type: 'tool/result', time: 1005, data: { turn: 1, step: 1, message: { role: 'tool', toolCallId: 's1', content: 'ok' }, meta: { kind_say: { parts: ['甲', '乙'] } } } },
  { seq: 7, type: 'turn/end', time: 1006, data: { turn: 1, reason: { kind: 'completed' } } },
];
const view = deriveTurnViews(events)[0];
assert.deepEqual(view.sayParts, ['甲', '乙']);
assert.equal(view.reaction, 'heart');
assert.equal(processStepsOf(view).some((step) => step.kind === 'tool'), false);
console.log('kind output: derivation ok');

const { parsePartialJson } = await import('../src/pages/kind/kindOutput.ts');
assert.deepEqual(parsePartialJson('{"parts":["甲"],}'), { parts: ['甲'] });
assert.deepEqual(parsePartialJson('{"parts":["甲","乙",],}'), { parts: ['甲', '乙'] });
console.log('kind output: trailing comma repair ok');
