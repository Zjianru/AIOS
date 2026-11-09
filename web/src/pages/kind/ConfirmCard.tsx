/**
 * 对话里的确认卡：从日志派生的事实（提议、决定、是否被替换）交给唯一的卡片入口渲染。
 * 确认和暂不都走 kindStore，由页面向服务发 card-act；卡片本身不知道具体动作。
 */

import { useMemo } from 'react';
import type { ConfirmCardFact } from './cards/model';
import { confirmCardEntryOf } from './cards/model';
import { Card } from './cards/Card';
import { useKindStore } from './kindStore';

export function ConfirmCard({ card }: { card: ConfirmCardFact }) {
  const confirmCard = useKindStore((state) => state.confirmCard);
  const dismissCard = useKindStore((state) => state.dismissCard);
  const envelope = useMemo(() => confirmCardEntryOf(card), [card]);
  return (
    <Card
      envelope={envelope}
      host={{
        answer: () => {},
        dismiss: () => {},
        confirmCard: (optionId) => confirmCard(card.callId, optionId),
        dismissCard: () => dismissCard(card.callId),
      }}
    />
  );
}
