/**
 * 对话里的任务结果卡：把 relay 消息的事实派生成卡片信封，交给卡片入口渲染。
 * 信封取不出来时不画卡（调用方只在解析成功时才走到这里）。
 */

import { useMemo } from 'react';
import type { TaskResultFact } from '@/data/taskResult';
import { taskResultCardOf } from '@/data/taskResult';
import { Card } from './cards/Card';

export function TaskResultCard({ fact }: { fact: TaskResultFact }) {
  const envelope = useMemo(() => taskResultCardOf(fact), [fact]);
  if (!envelope) return null;
  return <Card envelope={envelope} />;
}
