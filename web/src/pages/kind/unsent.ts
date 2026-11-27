//! 未落到日志的发送：排队视图与移除。

import type { TurnViewData } from './turnViews';
import { useKindStore } from './kindStore';
import { indexOf } from './viewUtils';

/** 没送到内核的本地消息：viewId -> { requestId, text, pastes, files, canvas }；重发复用同一个 requestId。 */
export const unsentSends = new Map<
  string,
  {
    requestId: string;
    text: string;
    pastes: string[];
    files: { id: string; name: string; bytes: number; mime?: string | null }[];
    canvases: { id: string; title: string }[];
    resultCards: { taskId: string; title: string; version: number }[];
  }
>();


export function removeView(viewId: string): void {
  useKindStore.setState((state) => {
    const views = state.views.filter((view) => view.id !== viewId);
    return { views, byId: indexOf(views) };
  });
}


/** 发不出去时在对话里留一条「没送到内核」的本地消息（重发成功后移除）。 */
export function addUnsentView(
  viewId: string,
  text: string,
  pastes: string[],
  files: { id: string; name: string; bytes: number; mime?: string | null }[],
  canvases: { id: string; title: string }[],
  resultCards: { taskId: string; title: string; version: number }[],
): void {
  useKindStore.setState((state) => {
    if (state.views.some((view) => view.id === viewId)) return {};
    const view: TurnViewData = {
      id: viewId,
      startedAt: null,
      durationMs: null,
      status: 'completed',
      user: {
        text,
        elements: [],
        pastes,
        files,
        images: [],
        objectRefs: [],
        quotes: [],
        canvas: canvases[0] ?? null,
        resultCard: resultCards[0] ?? null,
        rpcId: null,
      },
      taskResults: [],
      says: [],
      reasoning: null,
      tools: [],
      answer: null,
      thinkingSeconds: 0,
      tokens: { input: 0, output: 0 },
      model: '模型服务',
      cost: null,
      retry: null,
      failure: null,
      question: null,
      compaction: null,
      canvas: null,
      card: null,
      delivery: 'unsent',
      stoppedAt: null,
    };
    const views = state.views.concat(view).sort(
      (left, right) => Number(left.id.split('-')[1]) - Number(right.id.split('-')[1]),
    );
    return { views, byId: indexOf(views) };
  });
}
