/**
 * 对话里的提问卡（T017.2）：从 question/* 的事实派生卡片信封，交给唯一的卡片入口渲染。
 * 答题 / 不答仍然走原来的内部操作（kindStore 的 answer / dismiss）。
 */

import { useMemo } from 'react';
import type { QuestionView } from './kindStore';
import { useKindStore } from './kindStore';
import { Card } from './cards/Card';
import { questionCardEntryOf } from './cards/model';

export function QuestionCard({ turnId, question }: { turnId: string; question: QuestionView }) {
  const answer = useKindStore((state) => state.answer);
  const dismiss = useKindStore((state) => state.dismiss);
  // 派生失败（事实坏了）退回通用卡，不整卡消失。
  const envelope = useMemo(() => questionCardEntryOf(question), [question]);
  return (
    <Card
      envelope={envelope}
      host={{
        answer: (answers) => answer(turnId, answers),
        dismiss: () => dismiss(turnId),
      }}
    />
  );
}
