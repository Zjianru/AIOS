/**
 * Kind 对话里记忆小卡的状态与动作：状态按 ref 从记忆库读，变更后按推送/动作刷新。
 *
 * 状态只来自 MemoryRead 的 cards（服务按 ref 从库读）；动作只走 MemoryAction 的原子变更。
 * 页面不自己猜状态：卡片显示什么由库里的 status/outcome 决定。
 */

import { useEffect, useState } from 'react';
import { memoryAction, memoryRead } from '@/data/service';
import type { MemoryActionKind } from '@/data/service';

type CardStates = Record<string, string>;

let cache: CardStates = {};
const known = new Set<string>();
let loading: Promise<void> | null = null;
/** 读的过程中又有人要求重读：等在飞的那次收尾后再读一遍，别让旧结果盖掉新状态。 */
let dirty = false;
let queued = false;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

async function load(): Promise<void> {
  if (loading) {
    dirty = true;
    return loading;
  }
  const refs = [...known];
  if (refs.length === 0) return;
  loading = memoryRead({ refs })
    .then((response) => {
      for (const card of response.cards) cache[card.reference] = card.state;
      notify();
    })
    .catch(() => {
      // 读不出来就保持旧状态；动作的回执会另外报原因。
    })
    .finally(() => {
      loading = null;
      if (dirty) {
        dirty = false;
        void load();
      }
    });
  return loading;
}

function queueLoad(): void {
  if (queued) return;
  queued = true;
  queueMicrotask(() => {
    queued = false;
    void load();
  });
}

/** 订阅一批 ref 的卡状态；ref 变了才重读。 */
export function useMemoryCardStates(refs: Array<string | undefined>): CardStates {
  const key = refs.filter((ref): ref is string => Boolean(ref)).join('|');
  const [states, setStates] = useState<CardStates>(cache);
  useEffect(() => {
    const apply = () => setStates({ ...cache });
    for (const ref of key ? key.split('|') : []) known.add(ref);
    listeners.add(apply);
    apply();
    queueLoad();
    return () => {
      listeners.delete(apply);
    };
  }, [key]);
  return states;
}

/** 服务推来 memory/changed：丢缓存、按已知 ref 重读。 */
export function memoryChanged(): void {
  cache = {};
  notify();
  queueLoad();
}

/** 执行一个记忆动作；回执文案给卡片 toast。 */
export async function performMemoryAction(
  action: MemoryActionKind,
): Promise<{ message: string }> {
  const response = await memoryAction(action);
  memoryChanged();
  return { message: response.message };
}
