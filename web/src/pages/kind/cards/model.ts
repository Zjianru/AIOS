/**
 * 卡片机制（T017.2）：卡片信封、逐槽位的校验和降级、每类卡的注册项（schema / from / fallback）的纯数据模型。
 *
 * - 信封的 id、type、role、state、receipt 由程序定；模型的文字只进 slots。
 * - 每个槽位自己的规则都带 .catch，一个槽位坏了不牵连整张卡（spec 实现决定「校验和降级」）。
 * - 信封本身不合法（类型不认识、没有 id）才整卡退回 generic。
 * - 这个文件不依赖 React，node 单测（scripts/cards.test.mjs）直接 import。
 */

import { z } from 'zod';
// 工具卡的槽位从现有的派生函数来（terminalOf/diffOf/readOf/searchOf），不另写一份。
// 这个模块要被 node 单测直接 import：Node 的相对导入必须写扩展名，tsconfig 因此打开了
// allowImportingTsExtensions（只影响类型检查，不产出文件）。
import { diffOf, readOf, searchOf, terminalOf } from '../process/toolModel.ts';
import type { ToolStepView } from '../process/toolModel';

/* ───────── 信封 ───────── */

/** 注册表的键（spec :40；.2 登记 question 和 generic，其余随 .3/.4）。 */
export const CARD_TYPES = [
  'question',
  'terminal',
  'diff',
  'read',
  'search',
  'generic',
  'connection',
] as const;
export type CardType = (typeof CARD_TYPES)[number];

/** 卡片的角色：决定、状态、信息（颜色跟着角色和状态走，不由模型给）。 */
export const CARD_ROLES = ['decision', 'state', 'information'] as const;
export type CardRole = (typeof CARD_ROLES)[number];

export const CARD_STATES = ['pending', 'running', 'done', 'failed', 'stopped'] as const;
export type CardState = (typeof CARD_STATES)[number];

export const RECEIPT_OUTCOMES = ['success', 'partial', 'failed', 'cancelled'] as const;
export type ReceiptOutcome = (typeof RECEIPT_OUTCOMES)[number];

/** 回执：做过的决定留下的持久事实（答过、不答、被停了）。 */
export interface CardReceipt {
  outcome: ReceiptOutcome;
  summary: string;
}

/** 卡片信封：id/type/role/state/receipt 程序定，slots 的文字来自模型。 */
export interface CardEnvelope<Slots = unknown> {
  id: string;
  type: CardType;
  role: CardRole;
  state: CardState;
  slots: Slots;
  receipt?: CardReceipt;
}

/** 一次作答的形状（和 data/service 的 AskAnswer 一致；卡片模型不 import 页面数据层）。 */
export interface AnswerInput {
  id: string;
  selected: string[];
  custom?: string;
}

/** 卡片能触发的程序操作：按钮 id 和内部操作由程序定，模型给不了。 */
export interface CardHost {
  /** 提问卡：整批作答。 */
  answer: (answers: AnswerInput[]) => void;
  /** 提问卡：整批不答。 */
  dismiss: () => void;
}

/** 卡片视图的 props：信封、校验过的槽位、程序操作。 */
export interface CardViewProps<Slots> {
  envelope: CardEnvelope;
  slots: Slots;
  host: CardHost;
}

/** 槽位上限（spec :54：这两个数是常数，验收时截得不对可以调，不算改设计）。 */
export const LABEL_MAX = 20;
export const BODY_MAX = 600;

/** 一格正文：原文完整保留，超过上限标成折起来，点开看全文。 */
export interface BodySlot {
  text: string;
  folded: boolean;
}

/** 标签槽：小标题；不合法或超长退回默认值（默认值由每个槽位自己定，通常是空＝不显示）。 */
export type LabelSlot = string;

/* ───────── 提问卡 ───────── */

export interface QuestionOptionSlot {
  label: BodySlot;
  description?: BodySlot;
}

export interface QuestionSlot {
  id: string;
  header: LabelSlot;
  question: BodySlot;
  options: QuestionOptionSlot[];
  multiSelect: boolean;
}

export interface AnswerSlot {
  id: string;
  selected: string[];
  custom?: string;
}

export interface QuestionSlots {
  questions: QuestionSlot[];
  answers: AnswerSlot[];
}

/** 提问卡的事实：question/* 事件派生出来的 QuestionView（结构一致，不 import 页面模块）。 */
export interface QuestionCardFact {
  /** 问题批次 id（ask_user_question 调用的 call id），不用数组下标。 */
  batchId: string;
  status: 'pending' | 'answered' | 'dismissed' | 'void';
  questions: {
    id: string;
    question: string;
    header?: string;
    options?: { label: string; description?: string }[];
    multiSelect?: boolean;
  }[];
  answers?: { id: string; selected: string[]; custom?: string }[];
}

/* ───────── 通用卡 ───────── */

export interface GenericSlots {
  title: LabelSlot;
  body: BodySlot;
  /** 工具卡（T017.3）：不认识的工具展开后的输入 / 输出；认不出的信封没有这两项。 */
  input?: BodySlot;
  output?: BodySlot;
}

/* ───────── 槽位规则（每个都带 .catch） ───────── */

const EMPTY_BODY: BodySlot = { text: '', folded: false };

/** 字数按字符算（和「大段粘贴卡」的字数口径一致）。 */
function chars(text: string): number {
  return [...text].length;
}

/** 一段正文 → 正文槽：原文完整保留，超过上限标成折起来。 */
function bodyOf(text: string): BodySlot {
  return { text, folded: chars(text) > BODY_MAX };
}

const bodyInput = z
  .union([z.string(), z.object({ text: z.string() })])
  .transform((value): BodySlot => bodyOf(typeof value === 'string' ? value : value.text));

/**
 * 正文槽：不合法（不是字符串）就退回空，这一项不显示；超过 BODY_MAX 字标成折起来，原文不丢。
 * 也接受已经解出来的 {text}：卡片入口对同一个信封会再校验一次，解过的东西要能原样再过一遍。
 */
const bodySlot = bodyInput.catch(EMPTY_BODY);

/** 可缺的正文槽：不合法就是没有这一项，不塞一个空段（工具卡的输入 / 输出用）。 */
const optionalBodySlot = bodyInput.optional().catch(undefined);

/** 标签槽：不是字符串或超过 LABEL_MAX 字，退回这个槽位的默认值。 */
function labelSlot(fallbackValue: LabelSlot) {
  return z
    .string()
    .refine((text) => chars(text) <= LABEL_MAX, { message: '标签太长' })
    .catch(fallbackValue);
}

const optionSlotSchema = z.object({
  label: bodySlot,
  description: bodySlot.optional(),
});

/** 一道题：缺 id 或正文解不出来就整题去掉（catch(null) 由外层数组过滤）。 */
const questionSlotSchema = z.object({
  id: z.string().min(1),
  header: labelSlot(''),
  question: bodySlot,
  options: z
    .array(optionSlotSchema.nullable().catch(null))
    .default([])
    .transform((items) =>
      items.filter((item): item is QuestionOptionSlot => item !== null && item.label.text !== ''),
    ),
  multiSelect: z.boolean().catch(false),
});

const answerSlotSchema = z.object({
  id: z.string().min(1),
  selected: z.array(z.string()).catch([]),
  custom: z.string().optional(),
});

export const questionSlotsSchema = z.object({
  questions: z
    .array(questionSlotSchema.nullable().catch(null))
    .default([])
    .transform((items) =>
      items.filter((item): item is QuestionSlot => item !== null && item.question.text !== ''),
    ),
  answers: z
    .array(answerSlotSchema.nullable().catch(null))
    .default([])
    .transform((items) => items.filter((item): item is AnswerSlot => item !== null)),
});

export const genericSlotsSchema = z.object({
  title: labelSlot(''),
  body: bodySlot,
  input: optionalBodySlot,
  output: optionalBodySlot,
});

/* ───────── 信封本身的校验 ───────── */

/** 信封不合法时用的 id（没 id 才用；有 id 的保留，方便定位）。 */
export const FALLBACK_CARD_ID = 'unrecognized';

const receiptSchema = z.object({
  outcome: z.enum(RECEIPT_OUTCOMES).catch('failed'),
  summary: z.string().catch(''),
});

const envelopeSchema = z.object({
  id: z.string().min(1),
  type: z.enum(CARD_TYPES),
  role: z.enum(CARD_ROLES).catch('information'),
  state: z.enum(CARD_STATES).catch('pending'),
  slots: z.unknown(),
  // 回执坏了只丢回执，不把整张卡打回 generic（「字段坏了也不整卡报错」）。
  receipt: receiptSchema.optional().catch(undefined),
});

/**
 * 通用卡：认不出的信封退回这里。
 * - 本来就是通用卡、只是 id 坏了：留它自己的 slots（用户还看得出是什么卡）；
 * - 类型不认识或根本没信封：用通用卡自己的默认槽位，不把别类卡的槽位硬塞进来。
 */
export function genericCardOf(raw: unknown): CardEnvelope<GenericSlots> {
  const record =
    raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {};
  const id =
    typeof record.id === 'string' && record.id.length > 0 ? record.id : FALLBACK_CARD_ID;
  const parsed = genericSlotsSchema.safeParse(record.slots);
  const carrySlots = record.type === 'generic' && parsed.success;
  return {
    id,
    type: 'generic',
    role: 'information',
    state: 'pending',
    slots: carrySlots ? parsed.data : GENERIC_FALLBACK_SLOTS,
  };
}

/** 信封不合法（类型不认识、没有 id）：整卡退回 generic，不抛异常。 */
export function normalizeEnvelope(raw: unknown): CardEnvelope {
  const parsed = envelopeSchema.safeParse(raw);
  if (parsed.success) return parsed.data;
  return genericCardOf(raw);
}

/* ───────── 提问卡：从 question/* 的事实派生信封 ───────── */

function questionStateOf(status: QuestionCardFact['status']): CardState {
  if (status === 'pending') return 'pending';
  // 这一轮被停下：问题不再等回答。
  if (status === 'void') return 'stopped';
  return 'done';
}

/** 答过的回执文字：对话里的提问卡和工作过程里的提问卡共用一句。 */
function answeredSummary(count: number): string {
  return '你回答了 Kind 的 ' + String(count) + ' 个问题';
}

/** 回执文字和现在页面上的完全一样（QuestionCard 的 ClosedCard）。 */
function questionReceiptOf(fact: QuestionCardFact, count: number): CardReceipt | null {
  switch (fact.status) {
    case 'answered':
      return { outcome: 'success', summary: answeredSummary(count) };
    case 'dismissed':
      return { outcome: 'cancelled', summary: '你没回答 Kind 的问题；它知道你没答，照常收尾' };
    case 'void':
      return { outcome: 'cancelled', summary: '这一轮停下了，问题没再等回答' };
    default:
      return null;
  }
}

export function questionCardOf(fact: QuestionCardFact): CardEnvelope<QuestionSlots> | null {
  if (!fact || typeof fact !== 'object') return null;
  const parsed = questionSlotsSchema.safeParse({
    questions: fact.questions,
    answers: fact.answers ?? [],
  });
  if (!parsed.success) return null;
  const receipt = questionReceiptOf(fact, parsed.data.questions.length);
  return {
    id: typeof fact.batchId === 'string' ? fact.batchId : '',
    type: 'question',
    role: 'decision',
    state: questionStateOf(fact.status),
    slots: parsed.data,
    ...(receipt ? { receipt } : {}),
  };
}

/* ───────── 注册项 ───────── */

export interface CardDefinition<Fact, Slots> {
  /** 每个槽位一条规则，都带 .catch。 */
  schema: z.ZodType<Slots>;
  /** 从事实派生信封；取不出返回 null。 */
  from: (fact: Fact) => CardEnvelope<Slots> | null;
  /** slots 整块坏掉时的默认值（每个槽位的默认值）。 */
  fallback: Slots;
}

/** 信封 → 槽位：schema 过了就用解析结果，没过就整块退回默认值。卡片入口和单测共用。 */
export function resolveSlots<Fact, Slots>(
  definition: CardDefinition<Fact, Slots>,
  slots: unknown,
): Slots {
  const parsed = definition.schema.safeParse(slots);
  return parsed.success ? parsed.data : definition.fallback;
}

const QUESTION_FALLBACK_SLOTS: QuestionSlots = { questions: [], answers: [] };
const GENERIC_FALLBACK_SLOTS: GenericSlots = { title: '这张卡认不出来', body: EMPTY_BODY };

export const questionCard: CardDefinition<QuestionCardFact, QuestionSlots> = {
  schema: questionSlotsSchema,
  from: questionCardOf,
  fallback: QUESTION_FALLBACK_SLOTS,
};

export const genericCard: CardDefinition<unknown, GenericSlots> = {
  schema: genericSlotsSchema,
  from: genericCardOf,
  fallback: GENERIC_FALLBACK_SLOTS,
};

/* ───────── 工具卡（T017.3）：工具名 → 类型、槽位和信封 ───────── */

/**
 * 工具名 → 卡片类型（spec 实现决定：另有一张工具名对应卡片类型的表）。
 * write / edit / apply_patch 都是差异；glob / grep 都是搜索；没登记的工具退回通用卡。
 */
const TOOL_CARD_TYPES: Record<string, CardType> = {
  write: 'diff',
  edit: 'diff',
  apply_patch: 'diff',
  bash: 'terminal',
  read: 'read',
  glob: 'search',
  grep: 'search',
  ask_user_question: 'question',
};

export function toolCardTypeOf(name: string): CardType {
  return TOOL_CARD_TYPES[name] ?? 'generic';
}

/** 工具状态 → 信封状态（程序定，不由模型给）。 */
function toolCardStateOf(state: ToolStepView['state']): CardState {
  switch (state) {
    case 'running':
      return 'running';
    case 'done':
      return 'done';
    case 'error':
      return 'failed';
    case 'stopped':
      return 'stopped';
  }
}

/** 工具卡的派生事实：一条 tool/* 调用和会话工作目录（差异的路径要相对化）。 */
export interface ToolCardFact {
  step: ToolStepView;
  cwd: string;
}

/* 终端 */

export interface TerminalSlots {
  command: string;
  output: BodySlot;
  exitCode: number | null;
  signal: string | null;
}

const textSlot = z.string().catch('');
const integerSlot = z.number().int().catch(0);
const nullableNumberSlot = z.number().int().nullable().catch(null);

export const terminalSlotsSchema = z.object({
  command: textSlot,
  output: bodySlot,
  exitCode: nullableNumberSlot,
  signal: z.string().nullable().catch(null),
});

/* 差异 */

export interface DiffLineSlot {
  kind: 'add' | 'del' | 'ctx' | 'hunk';
  text: string;
  number: number | null;
}

export interface DiffFileSlot {
  path: string;
  isNew: boolean;
  lines: DiffLineSlot[];
  added: number;
  removed: number;
}

export interface DiffSlots {
  files: DiffFileSlot[];
}

const diffLineSchema = z.object({
  kind: z.enum(['add', 'del', 'ctx', 'hunk']).catch('ctx'),
  text: textSlot,
  number: nullableNumberSlot,
});

const diffFileSchema = z.object({
  path: textSlot,
  isNew: z.boolean().catch(false),
  lines: z
    .array(diffLineSchema.nullable().catch(null))
    .default([])
    .transform((items) => items.filter((item): item is DiffLineSlot => item !== null)),
  added: integerSlot,
  removed: integerSlot,
});

export const diffSlotsSchema = z.object({
  files: z
    .array(diffFileSchema.nullable().catch(null))
    .default([])
    .transform((items) => items.filter((item): item is DiffFileSlot => item !== null)),
});

/* 读文件 */

export interface ReadLineSlot {
  number: number;
  text: string;
}

export interface ReadSlots {
  path: string;
  lines: ReadLineSlot[];
  footer: string;
}

const readLineSchema = z.object({ number: integerSlot, text: textSlot });

export const readSlotsSchema = z.object({
  path: textSlot,
  lines: z
    .array(readLineSchema.nullable().catch(null))
    .default([])
    .transform((items) => items.filter((item): item is ReadLineSlot => item !== null)),
  footer: textSlot,
});

/* 搜索 */

export interface SearchRowSlot {
  number: number;
  text: string;
}

export interface SearchGroupSlot {
  path: string;
  rows: SearchRowSlot[];
}

export interface SearchSlots {
  header: string;
  groups: SearchGroupSlot[];
}

const searchGroupSchema = z.object({
  path: textSlot,
  rows: z
    .array(readLineSchema.nullable().catch(null))
    .default([])
    .transform((items) => items.filter((item): item is SearchRowSlot => item !== null)),
});

export const searchSlotsSchema = z.object({
  header: textSlot,
  groups: z
    .array(searchGroupSchema.nullable().catch(null))
    .default([])
    .transform((items) => items.filter((item): item is SearchGroupSlot => item !== null)),
});

const TERMINAL_FALLBACK_SLOTS: TerminalSlots = { command: '', output: EMPTY_BODY, exitCode: null, signal: null };
const DIFF_FALLBACK_SLOTS: DiffSlots = { files: [] };
const READ_FALLBACK_SLOTS: ReadSlots = { path: '', lines: [], footer: '' };
const SEARCH_FALLBACK_SLOTS: SearchSlots = { header: '', groups: [] };

/* 从 tool/* 的事实派生信封 */

export function terminalCardOf(step: ToolStepView): CardEnvelope<TerminalSlots> | null {
  const card = terminalOf(step);
  if (!card) return null;
  return {
    id: step.id,
    type: 'terminal',
    role: 'state',
    state: toolCardStateOf(step.state),
    slots: {
      command: card.command,
      output: bodyOf(card.output),
      exitCode: card.exitCode,
      signal: card.signal ?? null,
    },
  };
}

export function diffCardOf(fact: ToolCardFact): CardEnvelope<DiffSlots> | null {
  const files = diffOf(fact.step, fact.cwd);
  if (!files || files.length === 0) return null;
  return {
    id: fact.step.id,
    type: 'diff',
    role: 'state',
    state: toolCardStateOf(fact.step.state),
    slots: { files },
  };
}

export function readCardOf(fact: ToolCardFact): CardEnvelope<ReadSlots> | null {
  const card = readOf(fact.step, fact.cwd);
  if (!card) return null;
  return {
    id: fact.step.id,
    type: 'read',
    role: 'state',
    state: toolCardStateOf(fact.step.state),
    slots: { path: card.path, lines: card.lines, footer: card.footer },
  };
}

export function searchCardOf(step: ToolStepView): CardEnvelope<SearchSlots> | null {
  const card = searchOf(step);
  if (!card) return null;
  return {
    id: step.id,
    type: 'search',
    role: 'state',
    state: toolCardStateOf(step.state),
    slots: { header: card.header, groups: card.groups },
  };
}

/** 不认识的工具：输入 / 输出两段；输出照旧只取前 200 行。 */
export function genericToolCardOf(step: ToolStepView): CardEnvelope<GenericSlots> {
  const input = JSON.stringify(step.args, null, 2);
  const output = step.output ? step.output.split('\n').slice(0, 200).join('\n') : '';
  return {
    id: step.id,
    type: 'generic',
    role: 'state',
    state: toolCardStateOf(step.state),
    slots: {
      title: '',
      body: EMPTY_BODY,
      input: bodyOf(input),
      ...(output ? { output: bodyOf(output) } : {}),
    },
  };
}

/** 工作过程里的提问卡：问题从调用参数来，回答从工具结果的 answers JSON 来。 */
function answersOf(output: string): unknown[] {
  try {
    const parsed: unknown = JSON.parse(output);
    if (parsed && typeof parsed === 'object' && Array.isArray((parsed as { answers?: unknown }).answers)) {
      return (parsed as { answers: unknown[] }).answers;
    }
  } catch {
    // 不是回答的 JSON：当没答，回执照「没答」写。
  }
  return [];
}

export function askCardOf(step: ToolStepView): CardEnvelope<QuestionSlots> | null {
  const parsed = questionSlotsSchema.safeParse({
    questions: step.args.questions,
    answers: step.state === 'done' ? answersOf(step.output) : [],
  });
  if (!parsed.success || parsed.data.questions.length === 0) return null;
  // 答完了却解不出回答：退回通用卡，不凭空造一份回执。
  if (step.state === 'done' && parsed.data.answers.length === 0) return null;
  const state: CardState =
    step.state === 'running' ? 'pending' : step.state === 'done' ? 'done' : 'stopped';
  const receipt =
    state === 'done'
      ? { outcome: 'success' as const, summary: answeredSummary(parsed.data.questions.length) }
      : state === 'stopped'
        ? {
            outcome: 'cancelled' as const,
            summary:
              step.state === 'stopped'
                ? '这一轮停下了，问题没再等回答'
                : '你没回答 Kind 的问题；它知道你没答，照常收尾',
          }
        : null;
  return {
    id: step.id,
    type: 'question',
    role: 'decision',
    state,
    slots: parsed.data,
    ...(receipt ? { receipt } : {}),
  };
}

export const terminalCard: CardDefinition<ToolCardFact, TerminalSlots> = {
  schema: terminalSlotsSchema,
  from: (fact) => terminalCardOf(fact.step),
  fallback: TERMINAL_FALLBACK_SLOTS,
};

export const diffCard: CardDefinition<ToolCardFact, DiffSlots> = {
  schema: diffSlotsSchema,
  from: diffCardOf,
  fallback: DIFF_FALLBACK_SLOTS,
};

export const readCard: CardDefinition<ToolCardFact, ReadSlots> = {
  schema: readSlotsSchema,
  from: readCardOf,
  fallback: READ_FALLBACK_SLOTS,
};

export const searchCard: CardDefinition<ToolCardFact, SearchSlots> = {
  schema: searchSlotsSchema,
  from: (fact) => searchCardOf(fact.step),
  fallback: SEARCH_FALLBACK_SLOTS,
};

/**
 * 一次工具调用 → 卡片信封。类型不认识就是通用卡；类型认识但取不出槽位
 * （失败、停下、还没跑完）也退回通用卡的输入 / 输出，不整卡报错。
 */
export function toolCardOf(step: ToolStepView, cwd: string): CardEnvelope {
  const fact: ToolCardFact = { step, cwd };
  switch (toolCardTypeOf(step.name)) {
    case 'terminal':
      return terminalCard.from(fact) ?? genericToolCardOf(step);
    case 'diff':
      return diffCard.from(fact) ?? genericToolCardOf(step);
    case 'read':
      return readCard.from(fact) ?? genericToolCardOf(step);
    case 'search':
      return searchCard.from(fact) ?? genericToolCardOf(step);
    case 'question':
      return askCardOf(step) ?? genericToolCardOf(step);
    default:
      return genericToolCardOf(step);
  }
}

/** 登记表：卡片类型 → 一项 { schema, from, fallback }（view 在 cards/Card.tsx 合进来）。 */
export const cardDefinitions = {
  question: questionCard,
  terminal: terminalCard,
  diff: diffCard,
  read: readCard,
  search: searchCard,
  generic: genericCard,
};

/* ───────── 颜色（程序按角色和状态定） ───────── */

export interface CardTone {
  border: string;
  background: string;
  text: string;
}

const NEUTRAL_TONE: CardTone = { border: 'border-line', background: '', text: 'text-ink-3' };
const WAIT_TONE: CardTone = { border: 'border-wait/40', background: 'bg-wait-soft/40', text: 'text-wait' };
const RUN_TONE: CardTone = { border: 'border-run/40', background: 'bg-run-soft/40', text: 'text-run' };
const FAIL_TONE: CardTone = { border: 'border-fail/40', background: 'bg-fail-soft/40', text: 'text-fail' };

/**
 * 外框颜色只由程序按角色和状态定：模型只填 slots 的文字，给不了颜色。
 * - 要用户做决定、还在等：等待色（提问卡现在的样子）；
 * - 正在跑：运行色；失败：失败色；
 * - 答完 / 停下：次级文字色（和现在收成回执的样子一样）。
 */
export function cardTone(role: CardRole, state: CardState): CardTone {
  if (state === 'failed') return FAIL_TONE;
  if (state === 'running') return RUN_TONE;
  if (state === 'pending' && role === 'decision') return WAIT_TONE;
  return NEUTRAL_TONE;
}
