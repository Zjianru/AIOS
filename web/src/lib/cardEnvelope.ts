/**
 * 卡片信封与槽位规则：
 * 信封的词汇（类型/角色/状态/回执）、槽位上限、逐槽位的 zod 规则和注册项形状。
 *
 * 设置页从 data/connectionCard 取连接卡，不再反向 import 对话页的 cards/model；
 * 信封这套纯数据放这里，页面和数据层都能用。这个文件不依赖 React。
 */

import { z } from 'zod';

/* ───────── 信封 ───────── */

/** 注册表的键（spec :40）。 */
export const CARD_TYPES = [
  'question',
  'terminal',
  'diff',
  'read',
  'search',
  'generic',
  'connection',
  'confirm',
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

/* ───────── 槽位规则（每个都带 .catch） ───────── */

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

export const EMPTY_BODY: BodySlot = { text: '', folded: false };

/** 字数按字符算（和「大段粘贴卡」的字数口径一致）。 */
export function chars(text: string): number {
  return [...text].length;
}

/** 一段正文 → 正文槽：原文完整保留，超过上限标成折起来。 */
export function bodyOf(text: string): BodySlot {
  return { text, folded: chars(text) > BODY_MAX };
}

const bodyInput = z
  .union([z.string(), z.object({ text: z.string() })])
  .transform((value): BodySlot => bodyOf(typeof value === 'string' ? value : value.text));

/**
 * 正文槽：不合法（不是字符串）就退回空，这一项不显示；超过 BODY_MAX 字标成折起来，原文不丢。
 * 也接受已经解出来的 {text}：卡片入口对同一个信封会再校验一次，解过的东西要能原样再过一遍。
 */
export const bodySlot = bodyInput.catch(EMPTY_BODY);

/** 可缺的正文槽：不合法就是没有这一项，不塞一个空段（工具卡的输入 / 输出用）。 */
export const optionalBodySlot = bodyInput.optional().catch(undefined);

/** 标签槽：不是字符串或超过 LABEL_MAX 字，退回这个槽位的默认值。 */
export function labelSlot(fallbackValue: LabelSlot) {
  return z
    .string()
    .refine((text) => chars(text) <= LABEL_MAX, { message: '标签太长' })
    .catch(fallbackValue);
}

/** 不折的正文槽：一行字（连接卡的名字、时间、原因、工具卡的命令等）。 */
export const textSlot = z.string().catch('');

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
