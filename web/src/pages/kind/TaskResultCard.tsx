/**
 * 对话里的任务结果卡：把 relay 消息的事实派生成卡片信封，交给卡片入口渲染。
 * 信封取不出来时不画卡（调用方只在解析成功时才走到这里）。
 * 「接受」走 kindStore 的 card-act；回执由 card/decision 事件回填。
 */

import { useMemo } from 'react';
import type { TaskResultFact } from '@/data/taskResult';
import { taskResultCardOf } from '@/data/taskResult';
import { Card } from './cards/Card';
import { useKindStore } from './kindStore';

export function TaskResultCard({ fact }: { fact: TaskResultFact }) {
  const acceptResult = useKindStore((state) => state.acceptTaskResult);
  const judgeGate = useKindStore((state) => state.judgeGate);
  const envelope = useMemo(() => taskResultCardOf(fact), [fact]);
  if (!envelope) return null;
  return (
    <Card
      envelope={envelope}
      host={{
        answer: () => {},
        dismiss: () => {},
        acceptResult: (callId, reportId) => acceptResult(callId, reportId),
        // T054.2 把「要求修改」接到输入框；T054.1 先出按钮，点了不行动。
        modifyRequirement: () => {},
        judgeGate: (conclusion, note) => judgeGate(envelope.id, conclusion, note),
      }}
    />
  );
}
