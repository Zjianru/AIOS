/**
 * 任务结果卡：程序生成的 relay 消息画成卡，代替原来的消息气泡。
 *
 * 分工照 D68：类型、状态、颜色、按钮由程序定；模型只填标题、摘要、问题这些文字槽。
 * 状态文字只取固定词（有结果 / 待你核 / 没开跑 / 卡住了 / 失败 / 等报告），不拼具体内容，
 * 也永远不写「完成」。
 * 写任务（T030）：小标题「写任务」、固定行「改了 N 个文件（写入范围 X）」+ 清单 + bash 说明
 * + 工作树路径；当前要求版本的 ready 报告才是「待你核」，按钮「接受」由程序走 card-act，
 * 接受与否从日志里的 card/decision 回填（刷新后照样是「你已接受」）。
 */

import { useState } from 'react';
import {
  ArrowUpRight,
  Check,
  CirclePause,
  CircleSlash,
  CircleX,
  FilePen,
  FolderOpen,
  Hourglass,
  PlugZap,
  SearchCheck,
  Square,
} from 'lucide-react';
import { go } from '@/app/navigate';
import type { LucideIcon } from 'lucide-react';
import type { TaskCardState, TaskResultSlots } from '@/data/taskResult';
import type { CardViewProps } from './model';
import { BodyText } from './BodyText';
import { cn } from '@/lib/utils';

/** 状态键 → 图标和颜色（程序定，不由模型给）。 */
const STATE_TONE: Record<TaskCardState, { icon: LucideIcon; tone: string }> = {
  ready: { icon: SearchCheck, tone: 'bg-done-soft text-done' },
  // 写任务当前版本的 ready：卡状态是「待你核」，Task 阶段仍是「待检查」。
  review: { icon: CircleSlash, tone: 'bg-wait-soft text-wait' },
  blocked: { icon: CirclePause, tone: 'bg-wait-soft text-wait' },
  failed: { icon: CircleX, tone: 'bg-fail-soft text-fail' },
  waiting: { icon: Hourglass, tone: 'bg-run-soft text-run' },
  stopped: { icon: Square, tone: 'bg-hover text-ink-2' },
  interrupted: { icon: PlugZap, tone: 'bg-wait-soft text-wait' },
  // T038：派发卡（程序从 task_accept 的 tool/call + tool/result 派生）。
  dispatching: { icon: Hourglass, tone: 'bg-run-soft text-run' },
  dispatched: { icon: Hourglass, tone: 'bg-run-soft text-run' },
  // T041：任务库说「派出去了但不在跑」（停下这一轮等），和任务对话页头同一个词。
  pending: { icon: CirclePause, tone: 'bg-hover text-ink-2' },
};

function timeOf(seconds: number): string {
  if (!seconds) return '';
  const date = new Date(seconds * 1000);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

/** 写任务的改动清单：清单和固定说明都是服务给的事实，页面不自己数文件。 */
function Changed({ slots }: { slots: TaskResultSlots }) {
  return (
    <div className="mt-2 text-2 leading-5" data-kind="task-changed">
      <div className="text-1 text-ink-3">{slots.changedLine}</div>
      {slots.changed.length > 0 ? (
        <ul className="mt-0.5">
          {slots.changed.map((file) => (
            <li key={file} className="process-path flex items-center gap-1.5 text-ink-2">
              <FilePen className="size-3 shrink-0 text-ink-3" />
              {file}
            </li>
          ))}
        </ul>
      ) : (
        <div className="text-ink-3">没有经写类工具改过文件。</div>
      )}
      {slots.bashNote ? (
        <div className="mt-0.5 text-1 text-wait" data-kind="task-bash-note">
          {slots.bashNote}
        </div>
      ) : null}
      {slots.worktreePath ? (
        <div
          className="mt-0.5 flex items-start gap-1 text-1 text-ink-3"
          data-kind="task-worktree"
        >
          <FolderOpen className="mt-[1px] size-3 shrink-0" />
          <span className="min-w-0 break-all">工作树：{slots.worktreePath}</span>
        </div>
      ) : null}
    </div>
  );
}

export function TaskResultCardView({ envelope, slots, host }: CardViewProps<TaskResultSlots>) {
  const [accepting, setAccepting] = useState(false);
  const tone = STATE_TONE[slots.state];
  const Icon = tone.icon;
  const accept = () => {
    if (accepting) return;
    setAccepting(true);
    void Promise.resolve(host.acceptResult?.(envelope.id))
      .catch(() => {
        // 失败原因由 kindStore 提示；按钮恢复，能再点。
      })
      .finally(() => setAccepting(false));
  };
  return (
    <div
      className="mt-4 rounded-card border border-line-2 bg-raised/60 p-3"
      data-kind="task-result"
      data-state={slots.state}
    >
      <div className="flex items-center gap-1.5 text-1 text-ink-3">
        {slots.write ? <FilePen className="size-3.5" /> : <SearchCheck className="size-3.5" />}
        {slots.write ? '写任务' : '调查任务'}
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
      {slots.write ? <Changed slots={slots} /> : null}
      <div className="mt-3 flex items-center gap-2">
        {slots.state === 'review' ? (
          slots.accepted ? (
            <span
              className="flex items-center gap-1 text-2 text-done"
              data-kind="task-accepted"
            >
              <Check className="size-3.5" />
              你已接受
            </span>
          ) : (
            <button
              type="button"
              data-kind="task-accept"
              disabled={accepting}
              onClick={accept}
              className="rounded-btn bg-ink px-3 py-1 text-2 text-content transition-opacity hover:opacity-90 disabled:opacity-60"
            >
              接受
            </button>
          )
        ) : null}
        <button
          type="button"
          data-kind="open-task"
          disabled={!slots.taskId}
          onClick={() => go('taskchat:' + slots.taskId)}
          className="ml-auto flex items-center gap-1 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:opacity-50"
        >
          打开任务对话
          <ArrowUpRight className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
