// T017.2：卡片信封和注册表（spec 测试清单 1–5、7 的 node 部分）。
// 期望值来自 spec 的降级规则和现在页面上的回执文案，不来自组件输出。
import assert from 'node:assert/strict';
import { BODY_MAX, LABEL_MAX, resolveSlots } from '../src/lib/cardEnvelope.ts';
import { connectionCardOf } from '../src/data/connectionCard.ts';
import { parseTaskRelay, taskResultCardOf } from '../src/data/taskResult.ts';
import {
  cardDefinitions,
  confirmCardEntryOf,
  confirmCardOf,
  normalizeEnvelope,
  questionCardEntryOf,
  questionCardOf,
  toolCardOf,
  toolCardTypeOf,
} from '../src/pages/kind/cards/model.ts';
import { diffOf, parseExitStatus, readOf, searchOf, terminalOf } from '../src/pages/kind/process/toolModel.ts';

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

// 清单 2：小标题超过 20 字截断显示；不是字符串或空字符串 → 默认值（空，不显示）。
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
  exactly20,
  '超过 20 字截断到 20 字，不因为截断整段消失',
);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', header: '', question: '空字符串' }] }))
    .slots.questions[0].header,
  '',
  '空字符串用默认值',
);
assert.equal(
  questionEnvelope(questionFact({ questions: [{ id: 'q1', header: 42, question: '不是字符串' }] }))
    .slots.questions[0].header,
  '',
  '不是字符串用默认值',
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

/* 提问卡取不出信封（null）时退回通用卡，不整卡消失、不抛异常。 */
const questionFallback = questionCardEntryOf(null);
assert.equal(questionFallback.type, 'generic', 'null 事实退回通用卡');
assert.equal(normalizeEnvelope(questionFallback).type, 'generic', '能过卡片入口再校验');

/* ───────── T017.3 工具卡（spec 清单 6、8；票面 15 份样例） ───────── */

/**
 * 工具调用 + 结果 → 页面用的工具步骤。
 * 折法同 turnViews 的 tool/call、tool/result：参数从 arguments 解析，退出码从结果末尾取，
 * 结果失败记 error，meta 原样带过来（不另造事件形状）。
 */
function stepOf(call, result) {
  const output = result?.output ?? '';
  const status = parseExitStatus(output);
  return {
    kind: 'tool',
    id: call.callId,
    name: call.name,
    args: JSON.parse(call.arguments ?? '{}'),
    output,
    exitCode: result ? status.exitCode : null,
    signal: result ? status.signal : null,
    durationMs: result ? 1 : null,
    state: result ? (result.error ? 'error' : 'done') : 'running',
    meta: result?.meta ?? null,
  };
}

const CWD = '/w';

/** 工具步骤 → 信封 → 校验过的槽位；类型和 id 先断言，出不来直接红。 */
function toolCardEnvelope(step, expectedType, label) {
  const envelope = toolCardOf(step, CWD);
  assert.equal(envelope.type, expectedType, label + '：卡片类型');
  assert.equal(envelope.id, step.id, label + '：卡片 id 用调用 id，不用数组下标');
  const slots = resolveSlots(cardDefinitions[envelope.type], envelope.slots);
  return { envelope, slots };
}

// 清单 6：工具名映射逐条过；不认识的退回通用。
const TOOL_TYPES = {
  write: 'diff',
  edit: 'diff',
  apply_patch: 'diff',
  bash: 'terminal',
  read: 'read',
  glob: 'search',
  grep: 'search',
  ask_user_question: 'question',
  mystery: 'generic',
};
for (const [name, expected] of Object.entries(TOOL_TYPES)) {
  assert.equal(toolCardTypeOf(name), expected, name + ' → ' + expected);
}

/* 工作过程里的待答提问卡读模型参数 multi_select（不是 multiSelect）。 */
const askMulti = stepOf(
  {
    callId: 'call-ask-multi',
    name: 'ask_user_question',
    arguments: JSON.stringify({
      questions: [
        {
          id: 'q1',
          question: '哪些都算？',
          header: '',
          options: [{ label: '甲' }, { label: '乙' }],
          multi_select: true,
        },
      ],
    }),
  },
  null,
);
const askMultiCard = toolCardEnvelope(askMulti, 'question', '提问卡多选');
assert.equal(askMultiCard.slots.questions[0].multiSelect, true, 'multi_select 要映射成 multiSelect');

/* 终端：长 / 短 / 缺字段 */
const bashShort = stepOf(
  { callId: 'call-bash-1', name: 'bash', arguments: '{"command":"echo hi"}' },
  { output: 'hi\n[exit code: 0]' },
);
const terminalShort = toolCardEnvelope(bashShort, 'terminal', '终端短');
assert.equal(terminalShort.envelope.role, 'state');
assert.equal(terminalShort.envelope.state, 'done');
assert.equal(terminalShort.slots.command, 'echo hi');
assert.equal(terminalShort.slots.output.text, 'hi');
assert.equal(terminalShort.slots.output.folded, false);
assert.equal(terminalShort.slots.exitCode, 0);
// 槽位和派生函数给的一样。
const terminalFact = terminalOf(bashShort);
assert.equal(terminalShort.slots.command, terminalFact.command);
assert.equal(terminalShort.slots.output.text, terminalFact.output);
assert.equal(terminalShort.slots.exitCode, terminalFact.exitCode);
assert.equal(terminalShort.slots.signal, terminalFact.signal);

const longTerminalOutput = Array.from({ length: 120 }, (_, index) => '第 ' + String(index) + ' 行输出').join('\n');
const bashLong = stepOf(
  { callId: 'call-bash-2', name: 'bash', arguments: '{"command":"run"}' },
  { output: longTerminalOutput },
);
const terminalLong = toolCardEnvelope(bashLong, 'terminal', '终端长');
assert.equal(terminalLong.slots.output.folded, true, '正文槽超过 600 字折起来');
assert.equal(terminalLong.slots.output.text, longTerminalOutput, '原文完整保留');
assert.equal(terminalLong.slots.output.text, terminalOf(bashLong).output);

const bashMissing = stepOf({ callId: 'call-bash-3', name: 'bash', arguments: '{}' }, { output: '' });
const terminalMissing = toolCardEnvelope(bashMissing, 'terminal', '终端缺字段');
assert.equal(terminalMissing.slots.command, '');
assert.equal(terminalMissing.slots.output.text, '');
assert.equal(terminalMissing.slots.output.folded, false);

/* 差异：长 / 短 / 缺字段 */
const writeShort = stepOf(
  { callId: 'call-write-1', name: 'write', arguments: JSON.stringify({ file_path: '/w/notes.md', content: 'one\ntwo\n' }) },
  { output: 'Updated notes.md' },
);
const diffShort = toolCardEnvelope(writeShort, 'diff', '差异短');
assert.equal(diffShort.envelope.role, 'state');
assert.equal(diffShort.envelope.state, 'done');
assert.deepEqual(diffShort.slots.files, diffOf(writeShort, CWD));
assert.equal(diffShort.slots.files.length, 1);
assert.equal(diffShort.slots.files[0].path, 'notes.md', '路径相对会话工作目录');
assert.equal(diffShort.slots.files[0].added, 2);

// 长样例走 tool/result.meta（实际改动）；短样例走调用参数（打算改成什么）。
const longFileText = Array.from({ length: 30 }, (_, index) => 'line ' + String(index)).join('\n');
const writeLong = stepOf(
  { callId: 'call-write-2', name: 'write', arguments: JSON.stringify({ file_path: '/w/long.md', content: longFileText }) },
  { output: 'Updated long.md', meta: { diffs: [{ path: '/w/long.md', oldText: null, newText: longFileText }], operation: 'create' } },
);
const diffLong = toolCardEnvelope(writeLong, 'diff', '差异长');
assert.equal(diffLong.slots.files[0].path, 'long.md');
assert.equal(diffLong.slots.files[0].isNew, true);
assert.equal(diffLong.slots.files[0].lines.length, 30);
assert.equal(diffLong.slots.files[0].added, 30);

const writeMissing = stepOf(
  { callId: 'call-write-3', name: 'write', arguments: JSON.stringify({ file_path: '/w/empty.md' }) },
  { output: 'Updated empty.md' },
);
const diffMissing = toolCardEnvelope(writeMissing, 'diff', '差异缺字段');
assert.equal(diffMissing.slots.files[0].path, 'empty.md');
assert.deepEqual(diffMissing.slots.files[0].lines, [], '缺 content 就是没有行，不整卡报错');

/* 读文件：长 / 短 / 缺字段 */
const readShort = stepOf(
  { callId: 'call-read-1', name: 'read', arguments: JSON.stringify({ file_path: '/w/notes.md' }) },
  { output: '<path>notes.md</path>\n<type>file</type>\n<content>\n1: one\n2: two\n</content>' },
);
const readShortCard = toolCardEnvelope(readShort, 'read', '读文件短');
assert.deepEqual(readShortCard.slots, readOf(readShort, CWD));
assert.equal(readShortCard.slots.path, 'notes.md');
assert.equal(readShortCard.slots.lines.length, 2);

const readLong = stepOf(
  { callId: 'call-read-2', name: 'read', arguments: JSON.stringify({ file_path: '/w/long.md' }) },
  {
    output:
      '<content>\n' +
      Array.from({ length: 80 }, (_, index) => String(index + 1) + ': line ' + String(index)).join('\n') +
      '\n</content>',
  },
);
const readLongCard = toolCardEnvelope(readLong, 'read', '读文件长');
assert.equal(readLongCard.slots.lines.length, 80);
assert.deepEqual(readLongCard.slots, readOf(readLong, CWD));

const readMissing = stepOf(
  { callId: 'call-read-3', name: 'read', arguments: '{}' },
  { output: '<content>\n(empty file)\n</content>' },
);
const readMissingCard = toolCardEnvelope(readMissing, 'read', '读文件缺字段');
assert.equal(readMissingCard.slots.path, '');
assert.deepEqual(readMissingCard.slots.lines, []);
assert.equal(readMissingCard.slots.footer, '(empty file)');

/* 搜索：长 / 短 / 缺字段 */
const globShort = stepOf(
  { callId: 'call-glob-1', name: 'glob', arguments: '{"pattern":"*.ts"}' },
  { output: './a.ts\n./b.ts\n' },
);
const searchShort = toolCardEnvelope(globShort, 'search', '搜索短');
assert.deepEqual(searchShort.slots, searchOf(globShort));
assert.equal(searchShort.slots.header, '2 个文件');
assert.deepEqual(searchShort.slots.groups.map((group) => group.path), ['a.ts', 'b.ts']);

const globLong = stepOf(
  { callId: 'call-glob-2', name: 'glob', arguments: '{"pattern":"*.ts"}' },
  { output: Array.from({ length: 40 }, (_, index) => './f' + String(index) + '.ts').join('\n') },
);
const searchLong = toolCardEnvelope(globLong, 'search', '搜索长');
assert.equal(searchLong.slots.header, '40 个文件');
assert.equal(searchLong.slots.groups.length, 40);
assert.deepEqual(searchLong.slots, searchOf(globLong));

const grepMissing = stepOf(
  { callId: 'call-grep-3', name: 'grep', arguments: '{"pattern":"nope"}' },
  { output: 'no hits here' },
);
const searchMissing = toolCardEnvelope(grepMissing, 'search', '搜索缺字段');
assert.equal(searchMissing.slots.header, 'no hits here');
assert.deepEqual(searchMissing.slots.groups, []);

/* 通用：长 / 短 / 缺字段（不认识的工具） */
const mysteryShort = stepOf(
  { callId: 'call-mystery-1', name: 'mystery', arguments: '{"foo":"bar"}' },
  { output: 'done' },
);
const genericToolShort = toolCardEnvelope(mysteryShort, 'generic', '通用短');
assert.equal(genericToolShort.envelope.role, 'state');
assert.equal(genericToolShort.slots.title, '');
assert.equal(genericToolShort.slots.input.text, '{\n  "foo": "bar"\n}');
assert.equal(genericToolShort.slots.output.text, 'done');
assert.doesNotThrow(() => normalizeEnvelope(genericToolShort.envelope), '不认识的工具点开是通用卡，不报错');

const mysteryLong = stepOf(
  { callId: 'call-mystery-2', name: 'mystery', arguments: JSON.stringify({ text: 'x'.repeat(2000) }) },
  { output: 'done' },
);
const genericToolLong = toolCardEnvelope(mysteryLong, 'generic', '通用长');
assert.equal(genericToolLong.slots.input.folded, true);
assert.equal(
  genericToolLong.slots.input.text,
  '{\n  "text": "' + 'x'.repeat(2000) + '"\n}',
  '原文完整保留',
);

const mysteryMissing = stepOf({ callId: 'call-mystery-3', name: 'mystery', arguments: '{}' });
const genericToolMissing = toolCardEnvelope(mysteryMissing, 'generic', '通用缺字段');
assert.equal(genericToolMissing.envelope.state, 'running');
assert.equal(genericToolMissing.slots.input.text, '{}');
assert.equal(genericToolMissing.slots.output, undefined, '没有输出就不显示输出段');
assert.equal(genericToolMissing.slots.title, '');
assert.equal(genericToolMissing.slots.body.text, '');

/* 工作过程那张提问卡：从 tool/* 事件派生，id 用调用 id。 */
const askAnsweredStep = stepOf(
  {
    callId: 'call-ask-1',
    name: 'ask_user_question',
    arguments: JSON.stringify({
      questions: [{ id: 'q1', question: '先做哪个？', options: [{ label: '甲' }, { label: '乙' }] }],
    }),
  },
  { output: JSON.stringify({ answers: [{ id: 'q1', selected: ['甲'] }] }) },
);
const askAnswered = toolCardEnvelope(askAnsweredStep, 'question', '工作过程提问');
assert.equal(askAnswered.envelope.role, 'decision');
assert.equal(askAnswered.envelope.state, 'done');
assert.deepEqual(askAnswered.envelope.receipt, { outcome: 'success', summary: '你回答了 Kind 的 1 个问题' });
assert.equal(askAnswered.slots.questions[0].question.text, '先做哪个？');
assert.equal(askAnswered.slots.answers[0].selected[0], '甲');

const askDismissedStep = stepOf(
  {
    callId: 'call-ask-2',
    name: 'ask_user_question',
    arguments: JSON.stringify({ questions: [{ id: 'q1', question: '先做哪个？' }] }),
  },
  { output: 'ask_user_question was aborted before the user answered', error: true },
);
const askDismissed = toolCardEnvelope(askDismissedStep, 'question', '工作过程提问被撤');
assert.equal(askDismissed.envelope.state, 'stopped');
assert.deepEqual(askDismissed.envelope.receipt, {
  outcome: 'cancelled',
  summary: '你没回答 Kind 的问题；它知道你没答，照常收尾',
});

const askRunning = toolCardEnvelope(
  stepOf({
    callId: 'call-ask-3',
    name: 'ask_user_question',
    arguments: JSON.stringify({ questions: [{ id: 'q1', question: '先做哪个？' }] }),
  }),
  'question',
  '工作过程提问还在等',
);
assert.equal(askRunning.envelope.state, 'pending');
assert.equal(askRunning.envelope.receipt, undefined);

/* 槽位坏了只降级，不整卡报错（规则同 .2）。 */
const terminalBad = resolveSlots(cardDefinitions.terminal, {
  command: 42,
  output: { nope: true },
  exitCode: 'x',
  signal: 7,
});
assert.deepEqual(terminalBad, cardDefinitions.terminal.fallback);

const diffBad = resolveSlots(cardDefinitions.diff, {
  files: [
    { path: 42, lines: [{ kind: 'weird', text: 9, number: 'x' }], added: 'x' },
    'bad',
  ],
});
assert.equal(diffBad.files.length, 1, '坏的那一项去掉，好的留着');
assert.equal(diffBad.files[0].path, '');
assert.equal(diffBad.files[0].lines[0].kind, 'ctx');
assert.equal(diffBad.files[0].lines[0].text, '');
assert.equal(diffBad.files[0].lines[0].number, null);
assert.equal(diffBad.files[0].added, 0);

const readBad = resolveSlots(cardDefinitions.read, {
  path: 42,
  lines: [42, { text: '还留着' }],
  footer: 7,
});
assert.equal(readBad.path, '');
assert.equal(readBad.footer, '');
assert.equal(readBad.lines.length, 1);
assert.equal(readBad.lines[0].text, '还留着');
assert.equal(readBad.lines[0].number, 0);

const searchBad = resolveSlots(cardDefinitions.search, {
  header: 42,
  groups: [{ path: 7, rows: [42, { number: 3 }] }, 42],
});
assert.equal(searchBad.header, '');
assert.equal(searchBad.groups.length, 1);
assert.equal(searchBad.groups[0].path, '');
assert.equal(searchBad.groups[0].rows.length, 1);
assert.equal(searchBad.groups[0].rows[0].number, 3);
assert.equal(searchBad.groups[0].rows[0].text, '');

const genericBad = resolveSlots(cardDefinitions.generic, {
  title: 42,
  body: 42,
  input: { text: '输入还在' },
  output: 42,
});
assert.equal(genericBad.title, '');
assert.equal(genericBad.body.text, '');
assert.equal(genericBad.input.text, '输入还在');
assert.equal(genericBad.output, undefined);

/* ───────── T017.4 连接卡（票面：三种 Keyring 状态 + 长/短/缺字段） ───────── */

/** 连接卡的信封：出不来就是红（断言级，不是环境错）。 */
function connectionEnvelope(fact) {
  const envelope = connectionCardOf(fact);
  assert.ok(envelope, '连接卡要能从 Keyring 状态出信封');
  return envelope;
}

function connectionFact(overrides = {}) {
  return {
    id: 'deepseek',
    name: 'DeepSeek',
    provider: 'deepseek',
    providers: [{ id: 'deepseek', name: 'DeepSeek' }],
    key: { saved: false },
    check: { kind: 'never' },
    ...overrides,
  };
}

/* 三种 Keyring 状态各一个信封：状态和回执对得上。 */
const savedOnly = connectionEnvelope(
  connectionFact({ key: { saved: true, savedAt: '2026-11-08 13:00' } }),
);
assert.equal(savedOnly.type, 'connection');
assert.equal(savedOnly.role, 'decision', '连接卡是要用户做决定的卡');
assert.equal(savedOnly.id, 'deepseek', 'id 用连接 id，不用数组下标');
assert.equal(savedOnly.state, 'done', '已保存：决定做完（还差验证）');
assert.deepEqual(savedOnly.receipt, { outcome: 'success', summary: '已保存到 Keyring' });
assert.equal(savedOnly.slots.saved, true);
assert.equal(savedOnly.slots.savedAt, '2026-11-08 13:00');
assert.equal(savedOnly.slots.check, 'never', '还没验证过');

const available = connectionEnvelope(
  connectionFact({
    key: { saved: true, savedAt: '2026-11-08 13:00' },
    check: { kind: 'ok', at: '2026-11-08 13:01' },
  }),
);
assert.equal(available.state, 'done');
assert.deepEqual(available.receipt, { outcome: 'success', summary: '连接可用' });
assert.equal(available.slots.check, 'ok');
assert.equal(available.slots.checkedAt, '2026-11-08 13:01');

const unavailable = connectionEnvelope(
  connectionFact({
    key: { saved: true, savedAt: '2026-11-08 13:00' },
    check: { kind: 'fail', at: '2026-11-08 13:01', reason: '拒绝了这把密钥' },
  }),
);
assert.equal(unavailable.state, 'failed');
assert.deepEqual(unavailable.receipt, { outcome: 'failed', summary: '连接不可用：拒绝了这把密钥' });
assert.equal(unavailable.slots.reason, '拒绝了这把密钥');

// 还没保存：等用户填（pending），没有回执。
const unsaved = connectionEnvelope(connectionFact());
assert.equal(unsaved.state, 'pending');
assert.equal(unsaved.receipt, undefined, '还没做决定就没有回执');

// 正在验证：运行中，没有回执。
const checking = connectionEnvelope(
  connectionFact({ key: { saved: true }, check: { kind: 'checking' } }),
);
assert.equal(checking.state, 'running');
assert.equal(checking.receipt, undefined);

/* 长：多提供方、已保存、可用。 */
const longConnection = connectionEnvelope(
  connectionFact({
    providers: [
      { id: 'deepseek', name: 'DeepSeek' },
      { id: 'minimax-cn', name: 'MiniMax（国内站）' },
      { id: 'minimax', name: 'MiniMax（国际站）' },
    ],
    provider: 'minimax-cn',
    key: { saved: true, savedAt: '2026-11-08 13:00' },
    check: { kind: 'ok', at: '2026-11-08 13:01' },
  }),
);
assert.equal(longConnection.slots.providers.length, 3);
assert.deepEqual(
  longConnection.slots.providers.map((provider) => provider.id),
  ['deepseek', 'minimax-cn', 'minimax'],
  '提供方保持目录顺序',
);
assert.equal(longConnection.slots.providers[1].name, 'MiniMax（国内站）');
assert.equal(longConnection.slots.provider, 'minimax-cn');

/* 连接名超过标签上限：截断到 20 字显示，不整段消失。 */
const longNameConnection = connectionEnvelope(
  connectionFact({ name: exactly20 + '多出来的连接名' }),
);
assert.equal(longNameConnection.slots.name, exactly20, '连接名超过 20 字截断到 20 字');
assert.equal([...longNameConnection.slots.name].length, LABEL_MAX);

/* 短：单提供方、未保存。 */
const shortConnection = connectionEnvelope(connectionFact());
assert.equal(shortConnection.slots.providers.length, 1);
assert.equal(shortConnection.slots.saved, false);
assert.equal(shortConnection.receipt, undefined);

/* 缺字段：可选字段缺失、列表为空，也照样出信封，不整卡报错。 */
const missingFields = connectionEnvelope({ id: 'first-use' });
assert.equal(missingFields.type, 'connection');
assert.equal(missingFields.state, 'pending');
assert.deepEqual(missingFields.slots.providers, []);
assert.equal(missingFields.slots.name, '');
assert.equal(missingFields.slots.provider, '');
assert.equal(missingFields.slots.saved, false);
assert.equal(missingFields.slots.savedAt, '');
assert.equal(missingFields.slots.check, 'never');
assert.equal(missingFields.slots.checkedAt, '');
assert.equal(missingFields.slots.reason, '');
assert.equal(missingFields.slots.broken, false);

// Keyring 读不出来：整条连接按失败处理，别的字段照常降级。
const brokenKeyring = connectionEnvelope(
  connectionFact({ key: { saved: true }, broken: true }),
);
assert.equal(brokenKeyring.state, 'failed');
assert.equal(brokenKeyring.slots.broken, true);
assert.doesNotThrow(() => normalizeEnvelope(brokenKeyring), '连接卡的信封能过卡片入口');

// 没有 id：出不了信封，由卡片入口退回通用卡。
assert.equal(connectionCardOf({ id: '' }), null);

/* providers 是坏数组（不是数组）时退回空列表，连接卡仍出信封，不整卡 generic。 */
const badProviders = connectionEnvelope(connectionFact({ providers: { nope: true } }));
assert.equal(badProviders.type, 'connection');
assert.deepEqual(badProviders.slots.providers, []);
assert.equal(badProviders.state, 'pending');

// 槽位坏了逐项降级，不整卡报错。
const connectionBad = resolveSlots(cardDefinitions.connection, {
  name: 42,
  providers: [{ id: 'deepseek', name: 42 }, 42, { name: '没有 id' }],
  provider: 42,
  saved: 'yes',
  savedAt: 42,
  check: 'weird',
  checkedAt: 42,
  reason: 42,
  broken: 'yes',
});
assert.equal(connectionBad.name, '');
assert.equal(connectionBad.providers.length, 1, '坏的那项去掉，好的留着');
assert.equal(connectionBad.providers[0].name, '', '坏名字退回默认值');
assert.equal(connectionBad.provider, '');
assert.equal(connectionBad.saved, false);
assert.equal(connectionBad.savedAt, '');
assert.equal(connectionBad.check, 'never');
assert.equal(connectionBad.checkedAt, '');
assert.equal(connectionBad.reason, '');
assert.equal(connectionBad.broken, false);


/* ───────── 确认卡（T014.4）：单选、事实行、四种回执和派生 replaced ───────── */

function confirmFact(overrides = {}) {
  return {
    callId: 'call-card-1',
    proposal: {
      title: '换成哪个模型',
      brief: '选一个，点了才生效。',
      options: [
        {
          id: 'a',
          label: 'V4 Pro',
          description: '更强一点的模型',
          facts: [{ name: '模型', value: 'Flash → V4 Pro' }],
        },
        {
          id: 'b',
          label: 'Flash 最强思考',
          facts: [{ name: '推理强度', value: 'high → max' }],
        },
        { id: 'c', label: '丙', facts: [] },
      ],
    },
    decision: null,
    replaced: false,
    ...overrides,
  };
}

function confirmEnvelope(fact) {
  const envelope = confirmCardOf(fact);
  assert.ok(envelope, '确认卡要能从事实出信封');
  return envelope;
}

// 没点时：单选、三个选项、事实行来自服务端；卡片角色是 decision。
const pending = confirmEnvelope(confirmFact());
assert.equal(pending.type, 'confirm');
assert.equal(pending.role, 'decision');
assert.equal(pending.state, 'pending');
assert.equal(pending.receipt, undefined);
assert.equal(pending.slots.options.length, 3);
assert.deepEqual(pending.slots.options[0].facts, [{ name: '模型', value: 'Flash → V4 Pro' }]);
assert.equal(pending.slots.options[1].facts.length, 1);
assert.equal(pending.slots.options[0].description.text, '更强一点的模型');

// 确认：回执写选了哪一项和结果。
const confirmed = confirmEnvelope(
  confirmFact({
    decision: { phase: 'confirmed', option: 'b', data: { summary: '下一轮用 Flash（思考 max）' } },
  }),
);
assert.equal(confirmed.state, 'done');
assert.equal(confirmed.receipt.outcome, 'success');
assert.ok(confirmed.receipt.summary.includes('已执行'));
assert.ok(confirmed.receipt.summary.includes('Flash 最强思考'));
assert.ok(confirmed.receipt.summary.includes('下一轮用 Flash（思考 max）'));

// 暂不 / 已经变了 / 失败：三种回执。
const confirmDismissed = confirmEnvelope(confirmFact({ decision: { phase: 'dismissed' } }));
assert.equal(confirmDismissed.receipt.outcome, 'cancelled');
assert.ok(confirmDismissed.receipt.summary.includes('暂不'));
assert.equal(confirmEnvelope(confirmFact({ decision: { phase: 'refused', data: { summary: '模型没了' } } })).receipt.summary, '已经变了，没执行：模型没了');
const failed = confirmEnvelope(confirmFact({ decision: { phase: 'failed', data: { summary: '磁盘满了' } } }));
assert.equal(failed.state, 'failed');
assert.equal(failed.receipt.outcome, 'failed');
assert.equal(failed.receipt.summary, '没做成：磁盘满了');

// 派生 replaced：没动过的旧卡收成回执，不再能点。
const replaced = confirmEnvelope(confirmFact({ replaced: true }));
assert.equal(replaced.state, 'done');
assert.equal(replaced.receipt.outcome, 'cancelled');
assert.ok(replaced.receipt.summary.includes('替换'));

// 有决定就不是 replaced（决定优先）。
const decided = confirmEnvelope(
  confirmFact({ replaced: true, decision: { phase: 'dismissed' } }),
);
assert.ok(decided.receipt.summary.includes('暂不'), '已经点过的卡不显示被替换');

// 槽位降级：正文过长折起来；标签过长截断到上限、选项保留；坏的事实行去掉。
const degraded = confirmEnvelope(
  confirmFact({
    proposal: {
      title: exactly20 + '多出的标题',
      brief: '字'.repeat(BODY_MAX + 1),
      options: [
        { id: 'long', label: '长'.repeat(LABEL_MAX + 1), facts: [] },
        { id: 'ok', label: '短', facts: [{ name: '名称', value: '值' }, { name: 42, value: '坏' }, 7] },
      ],
    },
  }),
);
assert.equal(degraded.slots.title, exactly20, '标题超过标签上限截断到 20 字，不退空');
assert.equal([...degraded.slots.title].length, LABEL_MAX);
assert.equal(degraded.slots.brief.folded, true, '长正文折起来');
assert.equal(degraded.slots.brief.text.length, BODY_MAX + 1, '原文完整保留');
assert.deepEqual(
  degraded.slots.options.map((option) => option.id),
  ['long', 'ok'],
  '超长 label 的选项留下，空 label 才去掉',
);
assert.equal(degraded.slots.options[0].label, '长'.repeat(LABEL_MAX), '超长 label 截断到上限');
assert.deepEqual(degraded.slots.options[1].facts, [{ name: '名称', value: '值' }], '坏的事实行去掉');

// 回归：MiniMax-M2.7-highspeed（22 字）曾被标签槽退回空、整项丢掉。
// 现在超长标签截断到 20 字显示，id / 说明 / 事实行都保留；空标签和非字符串才丢。
const highspeedOption = {
  id: 'minimax-highspeed',
  label: 'MiniMax-M2.7-highspeed',
  description: '同一代模型的高速版',
  facts: [{ name: '模型', value: 'MiniMax-M3.1-Flash → MiniMax-M2.7-highspeed' }],
};
const clipped = confirmEnvelope(
  confirmFact({
    proposal: {
      title: '换成哪个模型',
      brief: '选一个。',
      options: [
        highspeedOption,
        { id: 'blank', label: '   ', facts: [] },
        { id: 'number', label: 42, facts: [] },
        { id: 'short', label: 'V4 Pro', facts: [] },
      ],
    },
  }),
);
assert.equal([...highspeedOption.label].length, 22, '回归样例是 22 字');
assert.deepEqual(
  clipped.slots.options.map((option) => option.id),
  ['minimax-highspeed', 'short'],
  '超长标签不丢；空标签和非字符串才丢',
);
const clippedOption = clipped.slots.options[0];
assert.equal(clippedOption.label, 'MiniMax-M2.7-highspe', '显示截断后的 label');
assert.equal([...clippedOption.label].length, LABEL_MAX);
assert.equal(clippedOption.description.text, '同一代模型的高速版', '说明保留');
assert.deepEqual(clippedOption.facts, highspeedOption.facts, '事实行保留');

// 刚好 20 字原样显示，不多截一个。
const exactly20Option = confirmEnvelope(
  confirmFact({ proposal: { title: '', brief: '', options: [{ id: 'exact', label: exactly20 }] } }),
);
assert.equal(exactly20Option.slots.options[0].label, exactly20);

// 整卡取不出：没有选项、不是对象；入口退回通用卡，不抛异常。
assert.equal(confirmCardOf(confirmFact({ proposal: { title: '空', brief: '', options: [] } })), null);
assert.equal(confirmCardOf(confirmFact({ proposal: '不是对象' })), null);
assert.doesNotThrow(() => confirmCardEntryOf(confirmFact({ proposal: '不是对象' })));
assert.equal(confirmCardEntryOf(confirmFact({ proposal: '不是对象' })).type, 'generic');
assert.doesNotThrow(() => resolveSlots(cardDefinitions.confirm, { options: '不是数组' }));

// ───────── 任务结果卡：relay JSON → 卡片信封 ─────────

function relayMessage(overrides = {}) {
  const body = {
    kind: 'task_report',
    task: { id: 'task-1', title: '查一下重复链接的判重规则', stage: 'ready_for_review' },
    origin: 'agent',
    state: 'ready',
    summary: '判重用的是原始链接。',
    ...overrides,
  };
  return {
    id: 'msg-relay-1',
    role: 'user',
    source: { kind: 'user', rpcId: 'task-report:task-1:report-1' },
    content: [{ type: 'text', text: JSON.stringify(body) }],
  };
}

function taskEnvelope(message, at = 100) {
  const fact = parseTaskRelay(message, at);
  assert.ok(fact, '合法的 relay 要解析出事实');
  const envelope = taskResultCardOf(fact);
  assert.ok(envelope, '结果卡要能从事实出信封');
  return envelope;
}

const cardReady = taskEnvelope(relayMessage());
assert.equal(cardReady.type, 'task-result');
assert.equal(cardReady.id, 'task-report:task-1:report-1');
assert.equal(cardReady.role, 'state');
assert.equal(cardReady.state, 'done');
assert.equal(cardReady.slots.title, '查一下重复链接的判重规则');
assert.equal(cardReady.slots.status, '有结果');
assert.equal(cardReady.slots.summary.text, '判重用的是原始链接。');
assert.equal(cardReady.slots.question, undefined);
assert.equal(cardReady.slots.at, 100);

// blocked 只看 origin：package 是没开跑，agent 是卡住了；问题进正文槽。
const cardBlockedAgent = taskEnvelope(
  relayMessage({
    state: 'blocked',
    origin: 'agent',
    summary: '缺一份只有你能给的版本号',
    question: '用哪个版本？',
  }),
);
assert.equal(cardBlockedAgent.slots.status, '卡住了');
assert.equal(cardBlockedAgent.slots.question.text, '用哪个版本？');

const cardBlockedPackage = taskEnvelope(
  relayMessage({
    state: 'blocked',
    origin: 'package',
    summary: '任务包缺：deepseek-flash 的凭证',
  }),
);
assert.equal(cardBlockedPackage.slots.status, '没开跑', '同样是 blocked，origin=package 是没开跑');
assert.equal(cardBlockedPackage.slots.question, undefined);

// 同一段文字换了 origin 就换状态：卡片不读摘要内容去判状态（D73）。
const sameAgent = taskEnvelope(
  relayMessage({ state: 'blocked', origin: 'agent', summary: '同一段话' }),
);
const samePackage = taskEnvelope(
  relayMessage({ state: 'blocked', origin: 'package', summary: '同一段话' }),
);
assert.equal(sameAgent.slots.status, '卡住了');
assert.equal(samePackage.slots.status, '没开跑');
assert.equal(sameAgent.slots.summary.text, samePackage.slots.summary.text);

const cardFailed = taskEnvelope(
  relayMessage({ state: 'failed', origin: 'agent', summary: '文档站打不开，已经试过两次。' }),
);
assert.equal(cardFailed.slots.status, '失败');
assert.equal(failed.state, 'failed');

const cardWaiting = taskEnvelope(
  relayMessage({ state: 'waiting', origin: 'executor', summary: '我还在查，别急' }),
);
assert.equal(cardWaiting.slots.status, '等报告');
assert.equal(cardWaiting.state, 'running');

// 卡片任何时候都不写「完成」。
for (const envelope of [cardReady, cardBlockedAgent, cardBlockedPackage, cardFailed, cardWaiting]) {
  assert.ok(!envelope.slots.status.includes('完成'));
  assert.ok(!envelope.slots.status.includes('成功'));
}

// 标题是标签槽：超过 20 字截断，不整段消失。
const longTitle = '一二三四五六七八九十一二三四五六七八九十多一点';
assert.equal([...longTitle].length, 23);
const cardLong = taskEnvelope(
  relayMessage({
    task: { id: 'task-2', title: longTitle, stage: 'needs_conditions' },
    summary: '字'.repeat(650),
  }),
);
assert.equal([...cardLong.slots.title].length, LABEL_MAX);
assert.equal(cardLong.slots.title, [...longTitle].slice(0, LABEL_MAX).join(''));
assert.equal(cardLong.slots.summary.folded, true, '超过正文上限标成折起来');
assert.equal([...cardLong.slots.summary.text].length, 650, '原文完整保留');

// 坏 JSON、没有前缀、不是 task_report、缺必填字段：退回文本（null），不抛异常。
const badJson = { ...relayMessage(), content: [{ type: 'text', text: '{"kind":"task_report",' }] };
assert.equal(parseTaskRelay(badJson, 1), null);

assert.equal(
  parseTaskRelay({ ...relayMessage(), source: { kind: 'user', rpcId: 'task-report-prefix-only' } }, 1),
  null,
  '不是 task-report: 前缀的不是结果卡',
);
assert.equal(
  parseTaskRelay({ ...relayMessage(), source: { kind: 'user', rpcId: 'other:1:2' } }, 1),
  null,
);
const otherKind = JSON.parse(relayMessage().content[0].text);
otherKind.kind = 'not_a_report';
assert.equal(
  parseTaskRelay({ ...relayMessage(), content: [{ type: 'text', text: JSON.stringify(otherKind) }] }, 1),
  null,
);
assert.equal(
  parseTaskRelay({
    ...relayMessage(),
    content: [{ type: 'text', text: '{"kind":"task_report","task":{},"origin":"agent","state":"ready","summary":"x"}' }],
  }, 1),
  null,
  '缺 task.id/title 的 JSON 画不出卡',
);
assert.equal(parseTaskRelay(null, 1), null);

// 登记表里有这张卡；槽位坏了只降级，不抛异常。
assert.ok(cardDefinitions['task-result']);
assert.doesNotThrow(() => resolveSlots(cardDefinitions['task-result'], { summary: '不是正文槽' }));

console.log('cards.test.mjs: all assertions passed');
