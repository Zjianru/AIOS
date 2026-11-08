/**
 * 卡片机制（T017.2）：卡片信封、逐槽位的校验和降级、每类卡的注册项（schema / from / fallback）的纯数据模型。
 *
 * - 信封的 id、type、role、state、receipt 由程序定；模型的文字只进 slots。
 * - 每个槽位自己的规则都带 .catch，一个槽位坏了不牵连整张卡（spec 实现决定「校验和降级」）。
 * - 信封本身不合法（类型不认识、没有 id）才整卡退回 generic。
 * - 这个文件不依赖 React，node 单测（scripts/cards.test.mjs）直接 import。
 */

import { z } from 'zod';

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

/** 槽位上限（spec :54：这两个数是常数，验收时截得不对可以调，不算改设计）。 */
export const LABEL_MAX = 20;
export const BODY_MAX = 600;

/** 图片/文字之外的一格正文：原文完整保留，超过上限标成折起来，点开看全文。 */
export interface BodySlot {
  text: string;
  folded: boolean;
}

/** 标签槽：小标题、按钮文案；不合法或超长退回默认值，不显示。 */
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

/** 提问卡的小标题（标签槽）默认不显示。 */
export function questionCardOf(fact: QuestionCardFact): CardEnvelope<QuestionSlots> | null {
  // T017.2 骨架：签名齐全，行为留给下一笔提交；单测在这里红。
  void fact;
  return null;
}

export function genericCardOf(raw: unknown): CardEnvelope<GenericSlots> {
  // T017.2 骨架：签名齐全，行为留给下一笔提交；单测在这里红。
  void raw;
  return {
    id: 'unrecognized',
    type: 'generic',
    role: 'information',
    state: 'pending',
    slots: { title: '', body: { text: '', folded: false } },
  };
}

/** 信封不合法（类型不认识、没有 id）：整卡退回 generic，不抛异常。 */
export function normalizeEnvelope(raw: unknown): CardEnvelope {
  // T017.2 骨架：签名齐全，行为留给下一笔提交；单测在这里红。
  void raw;
  return {
    id: 'unrecognized',
    type: 'generic',
    role: 'information',
    state: 'pending',
    slots: null,
  };
}

/** 信封 → 槽位：schema 过了就用解析结果，没过就整块退回默认值。卡片入口和单测共用。 */
export function resolveSlots<Fact, Slots>(
  definition: CardDefinition<Fact, Slots>,
  slots: unknown,
): Slots {
  // T017.2 骨架：签名齐全，行为留给下一笔提交；单测在这里红。
  void slots;
  return definition.fallback;
}

/** 外框的颜色：程序按角色和状态定，模型给不了。 */
export interface CardTone {
  border: string;
  background: string;
  text: string;
}

export function cardTone(role: CardRole, state: CardState): CardTone {
  // T017.2 骨架：签名齐全，颜色表随实现提交补上。
  void role;
  void state;
  return { border: 'border-line', background: '', text: 'text-ink-3' };
}

export const questionCard: CardDefinition<QuestionCardFact, QuestionSlots> = {
  schema: z.any(),
  from: questionCardOf,
  fallback: { questions: [], answers: [] },
};

export const genericCard: CardDefinition<unknown, GenericSlots> = {
  schema: z.any(),
  from: genericCardOf,
  fallback: { title: '这张卡认不出来', body: { text: '', folded: false } },
};

/** 登记表：卡片类型 → 一项 { schema, from, fallback }（view 在 cards/Card.tsx 合进来）。 */
export const cardDefinitions = {
  question: questionCard,
  generic: genericCard,
};
