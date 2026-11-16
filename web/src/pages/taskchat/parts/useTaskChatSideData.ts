//! 任务对话页的旁路数据：轮次视图、上游结果、排队可见输入、暂停横幅与 Esc 停一轮。

import { useEffect, useMemo, useState } from 'react';
import type { SendAttachmentRef, ServiceEvent } from '@/data/service';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { loadExecSettings } from '@/data/service';
import { turnLabels } from '@/pages/kind/kindStore';
import { deriveTurnViews } from '@/pages/kind/turnViews';
import { withReportedCompletion } from '../taskLive';
import { ASSIGNMENT_PREFIX, TASK_SAY_PREFIX, rpcIdOf, visibleTaskEvents } from './helpers';

export function useTaskChatSideData(options: {
  events: ServiceEvent[];
  task: TaskReadHeader | null;
  taskId: string | null;
  saying: boolean;
  queued: { inputId: string; text: string; pastes: string[]; attachments: SendAttachmentRef[] }[];
  refreshHeader: () => void;
  stopTurn: () => void;
}): {
  paused: boolean;
  views: ReturnType<typeof deriveTurnViews>;
  upstreamResults: { task_id: string; title: string; requirement_version: number; summary: string }[];
  queuedVisible: { inputId: string; text: string; pastes: string[]; attachments: SendAttachmentRef[] }[];
} {
  const { events, task, taskId, saying, queued, refreshHeader, stopTurn } = options;
  const [paused, setPaused] = useState(false);

  const views = useMemo(
    () =>
      withReportedCompletion(deriveTurnViews(visibleTaskEvents(events), turnLabels()), task?.reportedAt),
    [events, task],
  );

  const upstreamResults = useMemo(() => {
    for (const event of events) {
      if (event.type !== 'user/message') continue;
      const message = event.data?.message as
        | { source?: { rpcId?: string }; content?: { type?: string; text?: string }[] }
        | undefined;
      if (!String(message?.source?.rpcId ?? '').startsWith(ASSIGNMENT_PREFIX)) continue;
      const messageText = (message?.content ?? []).map((block) => block?.text ?? '').join('');
      try {
        const parsed = JSON.parse(messageText) as {
          upstream_results?: {
            task_id: string;
            title: string;
            requirement_version: number;
            summary: string;
          }[];
        };
        return parsed.upstream_results ?? [];
      } catch {
        return [];
      }
    }
    return [];
  }, [events]);

  useEffect(() => {
    if (!task?.restartPending) return;
    const timer = window.setInterval(() => void refreshHeader(), 1000);
    return () => window.clearInterval(timer);
 // refreshHeader 只用 taskId；跟着 restartPending 重新挂。
 // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task?.restartPending, taskId]);

  useEffect(() => {
    let live = true;
    void loadExecSettings()
      .then((view) => {
        if (live) setPaused(view.paused);
      })
      .catch(() => {
 // 读不到就不显示横幅；发送照旧由服务端闸门决定。
      });
    return () => {
      live = false;
    };
  }, [taskId, saying]);

  const queuedVisible = useMemo(() => {
    const seen = new Set<string>();
    for (const event of events) {
      const rpcId = rpcIdOf(event);
      if (rpcId?.startsWith(TASK_SAY_PREFIX)) seen.add(rpcId.slice(TASK_SAY_PREFIX.length));
    }
    return queued.filter((item) => !seen.has(item.inputId));
  }, [events, queued]);

  useEffect(() => {
    if (!task?.running) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      if (document.querySelector('[role="dialog"],[role="menu"]')) return;
      event.preventDefault();
      void stopTurn();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
 // stopTurn 只用 taskId，跟着 running 重新挂。
 // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task?.running, taskId]);

  return { paused, views, upstreamResults, queuedVisible };
}
