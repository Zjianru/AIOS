// T017.2：卡片信封和注册表（spec 测试清单 1–5、7 的 node 部分）。
// 期望值来自 spec 的降级规则和现在页面上的回执文案，不来自组件输出。
import assert from 'node:assert/strict';
import {
  BODY_MAX,
  LABEL_MAX,
  cardDefinitions,
  normalizeEnvelope,
  questionCardOf,
  resolveSlots,
} from '../src/pages/kind/cards/model.ts';

/** 提问卡的信封：出不来就是红（断言级，不是环境错）。 */
function questionEnvelope(fact) {
  const envelope = questionCardOf(fact);
  assert.ok(envelope, '提问卡要能从事实出信封');
  return envelope;
}

function questionFact(overrides = {}) {
  return {
    status: 'pending',
    batchId: 'call-ask-1',
    questions: [{ id: 'q1', question: '先做哪个？', options: [{ label: '甲' }, { label: '乙' }] }],
    ...overrides,
  };
}

// 清单 1：短、长、缺小标题、缺说明四份样例都能出信封。
const short = questionEnvelope(questionFact());
assert.equal(short.type, 'question');
assert.equal(short.id, 'call-ask-1', 'id 用问题批次 id，不用数组下标');
assert.equal(short.role, 'decision');
assert.equal(short.state, 'pending');
assert.equal(short.receipt, undefined, '还没答的卡没有回执');
assert.equal(short.slots.questions.length, 1);
assert.equal(short.slots.questions[0].question.text, '先做哪个？');
assert.equal(short.slots.questions[0].options.length, 2);

const long = questionEnvelope(
  questionFact({
    questions: [
      {
        id: 'q1',
        header: '选一个',
        question: '今天做哪件事？',
        multiSelect: true,
        options: [
          { label: '甲', description: '甲的说明' },
          { label: '乙', description: '乙的说明' },
        ],
      },
    ],
  }),
);
assert.equal(long.slots.questions[0].header, '选一个');
assert.equal(long.slots.questions[0].multiSelect, true);
assert.equal(long.slots.questions[0].options[0].description.text, '甲的说明');

assert.equal(questionEnvelope(questionFact()).slots.questions[0].header, '', '缺小标题就是不显示');
assert.equal(
  questionEnvelope(questionFact()).slots.questions[0].options[0].description,
  undefined,
  '缺说明就是不显示',
);

// 清单 2：小标题超过 20 字或不是字符串 → 默认值（空，不显示）。
const exactly20 = '一二三四五六七八九十一二三四五六七八九十';
assert.equal([...exactly20].length, 20);
assert.equal(LABEL_MAX, 20);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', header: exactly20, question: '刚好 20 字' }] }))
    .slots.questions[0].header,
  exactly20,
);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', header: exactly20 + '多', question: '超过 20 字' }] }))
    .slots.questions[0].header,
  '',
);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', header: 42, question: '不是字符串' }] }))
    .slots.questions[0].header,
  '',
);

// 清单 3：问题超过 600 字 → 标成折起来，原文完整保留。
const longQuestion = '问'.repeat(2000);
const folded = questionEnvelope(questionFact({ questions: [{ id: 'q1', question: longQuestion }] }));
assert.equal(folded.slots.questions[0].question.folded, true);
assert.equal(folded.slots.questions[0].question.text, longQuestion);
assert.equal(folded.slots.questions[0].question.text.length, 2000);
assert.equal(BODY_MAX, 600);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', question: '甲'.repeat(BODY_MAX) }] }))
    .slots.questions[0].question.folded,
  false,
  '刚好 600 字不折',
);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', question: '甲'.repeat(BODY_MAX + 1) }] }))
    .slots.questions[0].question.folded,
  true,
  '601 字折起来',
);

// 清单 4：缺 label 的选项去掉，其余保持原来的顺序。
const badOption = questionEnvelope(
  questionFact({
    questions: [
      {
        id: 'q1',
        question: '选一个',
        options: [{ label: '第一' }, { description: '没有 label' }, { label: '' }, { label: '第二' }],
      },
    ],
  }),
);
assert.deepEqual(
  badOption.slots.questions[0].options.map((option) => option.label.text),
  ['第一', '第二'],
);
// 一个槽位坏了只丢那一项：说明坏了，选项还在。
const badDescription = questionEnvelope(
  questionFact({ questions: [{ id: 'q1', question: '选一个', options: [{ label: '乙', description: 42 }] }] }),
);
assert.equal(badDescription.slots.questions[0].options[0].label.text, '乙');
assert.equal(badDescription.slots.questions[0].options[0].description.text, '');

// 清单 5：类型不认识、没有 id → generic，不抛异常；坏槽位整块退回默认值。
const unknownType = normalizeEnvelope({
  id: 'call-9',
  type: 'bogus',
  role: 'decision',
  state: 'pending',
  slots: {},
});
assert.equal(unknownType.type, 'generic');
assert.equal(unknownType.id, 'call-9', '能留的 id 留着');
const noId = normalizeEnvelope({ type: 'question', role: 'decision', state: 'pending', slots: {} });
assert.equal(noId.type, 'generic');
assert.equal(noId.id, 'unrecognized');
assert.doesNotThrow(() => normalizeEnvelope(null));
assert.doesNotThrow(() => normalizeEnvelope('不是信封'));
assert.deepEqual(
  resolveSlots(cardDefinitions.question, { questions: '不是数组' }),
  cardDefinitions.question.fallback,
  'slots 整块坏掉就用每个槽位的默认值',
);
const mixed = resolveSlots(cardDefinitions.question, {
  questions: [{ id: 'q1', question: '好题' }, { question: '没有 id' }],
});
assert.equal(mixed.questions.length, 1, '坏的那题去掉，好的留着');
assert.equal(mixed.questions[0].question.text, '好题');

// 通用卡也有 schema 样例：短、长、缺字段，缺字段不整卡报错。
const genericShort = resolveSlots(cardDefinitions.generic, { title: '通用卡', body: '一行' });
assert.equal(genericShort.title, '通用卡');
assert.equal(genericShort.body.text, '一行');
const genericLong = resolveSlots(cardDefinitions.generic, { title: '通用卡', body: '字'.repeat(2000) });
assert.equal(genericLong.body.folded, true);
assert.equal(genericLong.body.text.length, 2000);
const genericMissing = resolveSlots(cardDefinitions.generic, {});
assert.equal(genericMissing.title, '');
assert.equal(genericMissing.body.text, '');

// 清单 7：答过、不答、被停了的回执文字和现在页面上的一样。
const answered = questionEnvelope(
  questionFact({
    status: 'answered',
    questions: [
      { id: 'q1', question: '甲' },
      { id: 'q2', question: '乙' },
    ],
    answers: [
      { id: 'q1', selected: ['甲'] },
      { id: 'q2', selected: [], custom: '自己写的' },
    ],
  }),
);
assert.equal(answered.state, 'done');
assert.deepEqual(answered.receipt, { outcome: 'success', summary: '你回答了 Kind 的 2 个问题' });
assert.deepEqual(
  answered.slots.answers.map((answer) => answer.custom ?? answer.selected.join('、')),
  ['甲', '自己写的'],
);

const dismissed = questionEnvelope(questionFact({ status: 'dismissed' }));
assert.equal(dismissed.state, 'done');
assert.deepEqual(dismissed.receipt, {
  outcome: 'cancelled',
  summary: '你没回答 Kind 的问题；它知道你没答，照常收尾',
});
assert.equal(dismissed.slots.answers.length, 0);

const voided = questionEnvelope(questionFact({ status: 'void' }));
assert.equal(voided.state, 'stopped');
assert.deepEqual(voided.receipt, { outcome: 'cancelled', summary: '这一轮停下了，问题没再等回答' });

console.log('cards.test.mjs: all assertions passed');
