/**
 * 任务结果卡：程序生成的 relay 消息画成卡，代替原来的消息气泡。
 *
 * 分工照 D68：类型、状态、颜色、按钮由程序定；模型只填标题、摘要、问题这些文字槽。
 * 状态文字只取固定词（有结果 / 没开跑 / 卡住了 / 失败 / 等报告），不拼具体内容，也永远不写「完成」。
 * 本票只渲染「打开任务对话」，打开任务的动作留给下一票。
 */

import { ArrowUpRight, CirclePause, CircleX, Hourglass, SearchCheck } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { TaskReportState, TaskResultSlots } from '@/data/taskResult';
import type { CardViewProps } from './model';
import { BodyText } from './BodyText';
import { cn } from '@/lib/utils';

/** 状态键 → 图标和颜色（程序定，不由模型给）。 */
const STATE_TONE: Record<TaskReportState, { icon: LucideIcon; tone: string }> = {
  ready: { icon: SearchCheck, tone: 'bg-done-soft text-done' },
  blocked: { icon: CirclePause, tone: 'bg-wait-soft text-wait' },
  failed: { icon: CircleX, tone: 'bg-fail-soft text-fail' },
  waiting: { icon: Hourglass, tone: 'bg-run-soft text-run' },
};

function timeOf(seconds: number): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

export function TaskResultCardView({ slots }: CardViewProps<TaskResultSlots>) {
  const tone = STATE_TONE[slots.state];
  const Icon = tone.icon;
  return (
    <div
      className="mt-4 rounded-card border border-line-2 bg-raised/60 p-3"
      data-kind="task-result"
      data-state={slots.state}
    >
      <div className="flex items-center gap-1.5 text-1 text-ink-3">
        <SearchCheck className="size-3.5" />
        调查任务
        <span className="brand-type ml-auto">{timeOf(slots.at)}</span>
      </div>
      <div className="mt-2 flex min-w-0 items-start gap-2">
        <div
          className="min-w-0 flex-1 text-3 font-medium leading-5 text-ink"
          data-kind="task-title"
        >
          {slots.title}
        </div>
        <span
          className={cn(
            'inline-flex min-w-0 shrink-0 items-center gap-1 rounded-badge px-1.5 py-[1px] text-1',
            tone.tone,
          )}
          data-kind="task-state"
        >
          <Icon className="size-3 shrink-0" />
          <span className="truncate">{slots.status}</span>
        </span>
      </div>
      {slots.question ? (
        <div className="mt-2 border-l-2 border-wait/60 pl-2.5" data-kind="task-question">
          <div className="text-1 text-ink-3">它问你</div>
          <BodyText
            slot={slots.question}
            className="whitespace-pre-wrap break-words text-3 leading-5 text-ink"
          />
        </div>
      ) : null}
      <BodyText
        slot={slots.summary}
        className="mt-2 whitespace-pre-wrap break-words text-2 leading-5 text-ink-2"
      />
      <div className="mt-3 flex items-center gap-2">
        <button
          type="button"
          data-kind="open-task"
          className="ml-auto flex items-center gap-1 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
        >
          打开任务对话
          <ArrowUpRight className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
