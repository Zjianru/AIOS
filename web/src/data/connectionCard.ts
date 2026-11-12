/**
 * 连接卡：
 * 从 Keyring 状态派生卡片的信封和槽位。首次使用页和设置页共用；不依赖 React，
 * 也不再让设置页反向 import 对话页的 cards/model。
 */

import { z } from 'zod';
import type {
  CardDefinition,
  CardEnvelope,
  CardReceipt,
  CardState,
  LabelSlot,
} from '../lib/cardEnvelope.ts';
import { labelSlot, textSlot } from '../lib/cardEnvelope.ts';

/** Keyring 里一个连接的当前状态（首次使用页和设置页共用；结构上等同 ConnectionView）。 */
export interface ConnectionCardFact {
  /** 连接 id；首次使用还没保存过时用固定值。 */
  id: string;
  name?: string;
  /** 可选：还没接的提供方（首次使用页的按钮）。空列表也能出信封。 */
  providers?: { id: string; name: string }[];
  /** 当前选中的提供方。 */
  provider?: string;
  /** Keyring 里的密钥状态。 */
  key?: { saved: boolean; savedAt?: string };
  /** 最近一次保存时拉到的模型列表结果。 */
  check?: { kind: 'never' | 'ok' | 'fail'; at?: string; reason?: string };
  /** 保存时拉到了几个模型（ok 时第一行下面写「已拿到 N 个模型」）。 */
  modelCount?: number;
  /** Keyring 读不出来：所有连接一起停用（设置页）。 */
  broken?: boolean;
}

export interface ConnectionProviderSlot {
  id: string;
  name: LabelSlot;
}

/** 连接卡的槽位：程序从 Keyring 状态填，模型的文字进不来。 */
export interface ConnectionSlots {
  name: LabelSlot;
  providers: ConnectionProviderSlot[];
  provider: string;
  saved: boolean;
  savedAt: string;
  check: 'never' | 'ok' | 'fail';
  checkedAt: string;
  modelCount: number;
  reason: string;
  broken: boolean;
}

/** 每个连接槽位一条规则，都带 .catch：缺字段或坏字段只降级，不整卡报错。 */
const providerSlotSchema = z
  .object({
    id: z.string().min(1),
    name: labelSlot(''),
  })
  .nullable()
  .catch(null);

export const connectionSlotsSchema = z.object({
  name: labelSlot(''),
  providers: z
    .array(providerSlotSchema)
    .default([])
    .transform((items) => items.filter((item): item is ConnectionProviderSlot => item !== null))
    // providers 是坏数组（不是数组）时回空列表，连接卡仍出信封，不整卡 generic。
    .catch([]),
  provider: textSlot,
  saved: z.boolean().catch(false),
  savedAt: textSlot,
  check: z.enum(['never', 'ok', 'fail']).catch('never'),
  checkedAt: textSlot,
  modelCount: z.number().int().nonnegative().catch(0),
  reason: textSlot,
  broken: z.boolean().catch(false),
});

const CONNECTION_FALLBACK_SLOTS: ConnectionSlots = {
  name: '',
  providers: [],
  provider: '',
  saved: false,
  savedAt: '',
  check: 'never',
  checkedAt: '',
  modelCount: 0,
  reason: '',
  broken: false,
};

/**
 * 连接的状态由 Keyring 决定（程序定，不由模型给）：
 * 已保存是决定做完了，拉不到模型列表是 failed；还没保存是 pending。
 */
function connectionStateOf(fact: ConnectionCardFact): CardState {
  if (fact.broken) return 'failed';
  if (!fact.key?.saved) return 'pending';
  return fact.check?.kind === 'fail' ? 'failed' : 'done';
}

/** 回执和状态对得上：已保存 / 已拿到列表是 success，拉不到是 failed；还没保存没有回执。 */
function connectionReceiptOf(fact: ConnectionCardFact): CardReceipt | null {
  if (fact.broken) return { outcome: 'failed', summary: 'Keyring 读不出来，这个连接暂时不能用' };
  if (!fact.key?.saved) return null;
  switch (fact.check?.kind) {
    case 'ok':
      return { outcome: 'success', summary: '已拿到模型列表' };
    case 'fail': {
      const reason = typeof fact.check.reason === 'string' ? fact.check.reason : '';
      return { outcome: 'failed', summary: reason ? `拉不到模型列表：${reason}` : '拉不到模型列表' };
    }
    default:
      return { outcome: 'success', summary: '已保存到 Keyring' };
  }
}

/** Keyring 状态 → 连接卡信封；没有连接 id（取不出事实）返回 null。 */
export function connectionCardOf(fact: ConnectionCardFact): CardEnvelope<ConnectionSlots> | null {
  if (!fact || typeof fact !== 'object') return null;
  const id = typeof fact.id === 'string' ? fact.id : '';
  if (!id) return null;
  const parsed = connectionSlotsSchema.safeParse({
    name: fact.name,
    providers: fact.providers,
    provider: fact.provider,
    saved: fact.key?.saved === true,
    savedAt: fact.key?.savedAt,
    check: fact.check?.kind,
    checkedAt: fact.check?.at,
    reason: fact.check?.reason,
    modelCount: fact.modelCount,
    broken: fact.broken === true,
  });
  if (!parsed.success) return null;
  const state = connectionStateOf(fact);
  const receipt = connectionReceiptOf(fact);
  return {
    id,
    type: 'connection',
    role: 'decision',
    state,
    slots: parsed.data,
    ...(receipt ? { receipt } : {}),
  };
}

export const connectionCard: CardDefinition<ConnectionCardFact, ConnectionSlots> = {
  schema: connectionSlotsSchema,
  from: connectionCardOf,
  fallback: CONNECTION_FALLBACK_SLOTS,
};
