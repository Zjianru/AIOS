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
  Terminal,
} from 'lucide-react';
import { go } from '@/app/navigate';
import { usageResumeLine } from '@/data/usageWait.ts';
import { useKindStore } from '@/pages/kind/kindStore';
import type { LucideIcon } from 'lucide-react';
import type { TaskCardState, TaskResultSlots } from '@/data/taskResult';
import type { CardViewProps } from './model';
import { previewOf } from './BodyText';
import { Markdown } from '@/pages/kind/Markdown';
import { cn } from '@/lib/utils';

/** T059 F1：卡里由模型/程序写的正文按 Markdown 渲染；折起时仍按原上限给预览。 */
function RichBody({ slot, className }: { slot: TaskResultSlots['summary']; className?: string }) {
  const [open, setOpen] = useState(false);
  if (!slot.text) return null;
  const folded = slot.folded && !open;
  return (
    <div className={className} data-kind="task-rich-body">
      <Markdown text={folded ? previewOf(slot.text) : slot.text} />
      {folded ? (
        <button
          type="button"
          data-action="card-expand"
          onClick={() => setOpen(true)}
          className="mt-1 text-1 text-ink-3 transition-colors hover:text-ink"
        >
          展开全文
        </button>
      ) : null}
    </div>
  );
}

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

function programDuration(ms: number): string {
  return ms >= 1000 ? (ms / 1000).toFixed(ms >= 10_000 ? 0 : 1) + ' 秒' : ms + ' 毫秒';
}

function programCheckWord(check: TaskResultSlots['programGate'] extends undefined
  ? never
  : NonNullable<TaskResultSlots['programGate']>['checks'][number]): { text: string; tone: string } {
  if (check.outcome === 'timed_out') return { text: '超时', tone: 'text-wait' };
  if (check.outcome === 'spawn_failed') return { text: '起不来', tone: 'text-fail' };
  if (check.outcome === 'stopped') return { text: '被停', tone: 'text-ink-3' };
  if (check.outcome === 'skipped') return { text: '没跑', tone: 'text-ink-3' };
  if (check.exit_code === 0) return { text: '退出码 0', tone: 'text-done' };
  return { text: '退出码 ' + String(check.exit_code ?? '?'), tone: 'text-fail' };
}

function ProgramChecks({ program }: { program: NonNullable<TaskResultSlots['programGate']> }) {
  return (
    <div className="mt-2 space-y-1.5" data-kind="program-checks">
      {program.commands.map((command, index) => {
        const check = program.checks[index];
        const word = check ? programCheckWord(check) : { text: '没跑', tone: 'text-ink-3' };
        const bad =
          check !== undefined &&
          (check.exit_code !== 0 || check.outcome === 'timed_out' || check.outcome === 'spawn_failed');
        return (
          <div
            key={index}
            className="min-w-0 text-2 leading-5"
            data-kind="program-check"
            data-exit={check?.exit_code ?? 'none'}
          >
            <div className="flex min-w-0 items-center gap-2">
              <Terminal className="size-3 shrink-0 text-ink-3" />
              <code className="process-path min-w-0 flex-1 truncate text-ink-2" title={command.command}>
                {command.command}
              </code>
              <span className={cn('shrink-0 text-1', word.tone)}>{word.text}</span>
              {check && check.duration_ms > 0 ? (
                <span className="brand-type shrink-0 text-1 text-ink-3">
                  {programDuration(check.duration_ms)}
                </span>
              ) : null}
            </div>
            {bad && check?.tail ? (
              <pre
                className="mt-1 max-h-[120px] overflow-auto whitespace-pre-wrap break-words rounded-btn bg-hover/60 px-2 py-1 font-mono text-1 leading-4 text-ink-2"
                data-kind="program-tail"
              >
                {check.tail}
              </pre>
            ) : null}
            {bad && check?.spill ? (
              <div className="mt-0.5 text-1 text-ink-3" data-kind="program-spill">
                完整输出：<span className="process-path">{check.spill}</span>
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

/** D115：结果卡上的「你说的」：默认一行，点一下看整句。 */
function SaidLine({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <button
      type="button"
      onClick={() => setOpen((value) => !value)}
      className={cn(
        'mt-1 block w-full min-w-0 text-left text-2 leading-5 text-ink-3 transition-colors hover:text-ink-2',
        !open && 'truncate',
      )}
      data-kind="task-said-line"
      title={open ? undefined : text}
    >
      你说的「<span className="text-ink-2">{text}</span>」
    </button>
  );
}

export function TaskResultCardView({ envelope, slots, host }: CardViewProps<TaskResultSlots>) {
  const [accepting, setAccepting] = useState(false);
  const [judging, setJudging] = useState(false);
  const [writingFix, setWritingFix] = useState(false);
  const [fixNote, setFixNote] = useState('');
  const program = slots.programGate;
  const tone = program
    ? program.conclusion === 'pass'
      ? STATE_TONE.ready
      : program.conclusion === 'needs_fix'
        ? STATE_TONE.review
        : STATE_TONE.waiting
    : STATE_TONE[slots.state];
  const Icon = program ? Terminal : tone.icon;
  const composerResultCard = useKindStore((state) => state.composerResultCard);
  const inComposer =
    composerResultCard !== null &&
    composerResultCard.taskId === slots.taskId &&
    composerResultCard.version === slots.reportVersion;
  const gate = program ? undefined : slots.gate;
  // 门自己的结果卡（等你判/你判了）不出接受；被把关任务卡（waiting）门判通过后要出接受。
  const gateSelf = gate?.state === 'pending_verdict';
  const gatePending = gate?.state === 'pending_verdict' && !slots.verdict && !slots.verdictChanged;
  const accept = () => {
    if (accepting) return;
    setAccepting(true);
    void Promise.resolve(host.acceptResult?.(envelope.id, slots.reportId || undefined))
      .catch(() => {
        // 失败原因由 kindStore 提示；按钮恢复，能再点。
      })
      .finally(() => setAccepting(false));
  };
  const judge = (conclusion: 'pass' | 'needs_fix', note?: string) => {
    if (judging || !gatePending) return;
    setJudging(true);
    void Promise.resolve(host.judgeGate?.(conclusion, note))
      .catch(() => {
        // 失败原因由 kindStore 提示；按钮恢复，能再点。
      })
      .finally(() => setJudging(false));
  };
  return (
    <div
      className="mt-4 rounded-card border border-line-2 bg-raised/60 p-3"
      data-kind="task-result"
      data-state={slots.state}
      data-acceptable={slots.acceptable ? 'true' : 'false'}
    >
      <div className="flex items-center gap-1.5 text-1 text-ink-3">
        {slots.write ? <FilePen className="size-3.5" /> : <SearchCheck className="size-3.5" />}
        {program
          ? '程序门 · 自动检查' + (program.commands_version > 1 ? ' · 第 ' + program.commands_version + ' 版命令' : '')
          : gate
            ? '里程碑 · 只读核查'
            : slots.write
              ? '写任务'
              : '调查任务'}
        {!program && !gate && slots.reportVersion > 1
          ? ' · 第 ' + slots.reportVersion + ' 版要求'
          : null}
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
      {slots.usageResumeAt === undefined ? null : (
        <div className="mt-1 text-1 text-wait" data-kind="task-usage-wait">
          {usageResumeLine(slots.usageResumeAt)}
        </div>
      )}
      {slots.quote && !slots.gate && !slots.programGate ? (
        <SaidLine
          text={[slots.quote.text]
            .concat((slots.laterQuotes ?? []).map((item) => '第 ' + item.version + ' 版：' + item.text))
            .join(' ／ ')}
        />
      ) : null}
      {gate?.state === 'pending_verdict' && gate.guarded && gate.candidate ? (
        <div className="mt-1 text-2 leading-5 text-ink-2" data-kind="gate-candidate">
          核查的是「{gate.guarded.title}」第 {gate.candidate.ordinal} 次交的结果
        </div>
      ) : null}
      {slots.verdictChanged ? (
        <div className="mt-2 text-2 leading-5 text-fail" data-kind="gate-changed">
          已经变了：没有记判定
        </div>
      ) : null}
      {slots.verdict ? (
        <div className="mt-2" data-kind="gate-verdict">
          {/* 用户判定原话按纯文本显示，保留换行；Markdown 会改写 * _ # 这类字符。 */}
          {slots.verdictNote ? (
            <div
              className="mb-1 whitespace-pre-wrap break-words border-l-2 border-line-2 pl-2.5 text-2 leading-5 text-ink-2"
              data-kind="gate-note"
            >
              {slots.verdictNote}
            </div>
          ) : null}
          <div className="text-2 text-ink-2">
            你判了{slots.verdict === 'pass' ? '通过' : '需修正'}
            {slots.verdictAt ? ' · ' + timeOf(slots.verdictAt) : ''}
          </div>
          <div className="mt-0.5 text-1 text-ink-3">
            {slots.verdict === 'pass'
              ? '被把关任务到「任务要求已满足」，后面的任务按顺序开跑。'
              : '已原样交给 Kind；它会让被把关任务在原要求里接着改。'}
          </div>
        </div>
      ) : null}
      {slots.question ? (
        <div className="mt-2 border-l-2 border-wait/60 pl-2.5" data-kind="task-question">
          <div className="text-1 text-ink-3">它问你</div>
          <RichBody slot={slots.question} className="break-words text-3 leading-5 text-ink" />
        </div>
      ) : null}
      <RichBody slot={slots.summary} className="mt-2 break-words text-2 leading-5 text-ink-2" />
      {program ? <ProgramChecks program={program} /> : null}
      {gate?.state === 'waiting' && gate.recheck ? (
        <div className="mt-1 text-1 text-done" data-kind="gate-recheck">
          核查自动重跑：{gate.title}
          {gate.program ? '（程序门，全部退出码为 0 就通过）' : '（里程碑，结果由你判）'}
        </div>
      ) : slots.autoStarted.length > 0 ? (
        <div className="mt-1 text-1 text-done" data-kind="task-auto-started">
          已自动开跑：{slots.autoStarted.join('、')}
          {gate?.state === 'waiting'
            ? gate.program
              ? '（程序门，全部退出码为 0 就通过）'
              : '（里程碑，结果由你判）'
            : ''}
        </div>
      ) : null}
      {program ? (
        <div
          className={cn('mt-2 text-1', program.conclusion === 'pass' ? 'text-done' : 'text-ink-3')}
          data-kind="program-next"
        >
          {program.conclusion === 'pass'
            ? slots.autoStarted.length > 0
              ? '后面的任务已自动开跑：' + slots.autoStarted.join('、')
              : '全部退出码为 0：程序判了通过'
            : program.conclusion === 'needs_fix'
              ? '已交给 Kind，让被把关任务照失败的输出接着改；改完程序再跑一遍'
              : '未确定不算通过，也不算有错；已交给 Kind，重跑还是换命令由你定'}
        </div>
      ) : null}
      {slots.upstreamChanged ? (
        <div className="mt-1 text-1 text-wait" data-kind="upstream-changed">
          上游有新结果，待按新结果核对
        </div>
      ) : null}
      {slots.write ? <Changed slots={slots} /> : null}
      {gatePending ? (
        <div className="mt-3" data-kind="gate-actions">
          {writingFix ? (
            <div className="rounded-card border border-line-2 bg-raised/60 p-2.5">
              <textarea
                data-kind="gate-note-input"
                value={fixNote}
                autoFocus
                onChange={(event) => setFixNote(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') {
                    setWritingFix(false);
                    return;
                  }
                  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
                    event.preventDefault();
                    judge('needs_fix', fixNote.trim() ? fixNote : undefined);
                  }
                }}
                placeholder="哪里不对、要怎么改？不写就按核查的发现改。"
                rows={3}
                className="w-full resize-y rounded-btn border border-line-2 bg-content px-2 py-1.5 text-2 leading-5 text-ink outline-none focus:border-ink-3/50"
              />
              <div className="mt-1 text-1 leading-4 text-ink-3">
                交给 Kind，让它在原要求里接着改；要改要求，回 Kind 对话说。
              </div>
              <div className="mt-2 flex items-center justify-end gap-2">
                <button
                  type="button"
                  data-kind="gate-needs-fix-cancel"
                  disabled={judging}
                  onClick={() => setWritingFix(false)}
                  className="rounded-btn px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:opacity-60"
                >
                  取消
                </button>
                <button
                  type="button"
                  data-kind="gate-needs-fix-submit"
                  disabled={judging}
                  onClick={() => judge('needs_fix', fixNote.trim() ? fixNote : undefined)}
                  className="rounded-btn bg-ink px-3 py-1 text-2 text-content transition-opacity hover:opacity-90 disabled:opacity-60"
                >
                  判需修正
                </button>
              </div>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              <button
                type="button"
                data-kind="gate-pass"
                disabled={judging}
                onClick={() => judge('pass')}
                className="rounded-btn bg-ink px-3 py-1 text-2 text-content transition-opacity hover:opacity-90 disabled:opacity-60"
              >
                通过
              </button>
              <button
                type="button"
                data-kind="gate-needs-fix"
                disabled={judging}
                onClick={() => setWritingFix(true)}
                className="rounded-btn border border-line-2 px-3 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:opacity-60"
              >
                需修正
              </button>
            </div>
          )}
        </div>
      ) : null}
      <div className="mt-3 flex items-center gap-2">
        {inComposer ? (
          <span
            data-kind="result-card-in-composer"
            className="rounded-badge bg-hover px-2 py-1 text-2 text-ink-2"
          >
            已带进输入框
          </span>
        ) : null}
        {!gateSelf && slots.accepted ? (
          <div className="min-w-0" data-kind="task-accepted">
            <div className="flex items-center gap-1 text-2 text-done">
              <Check className="size-3.5" />
              你接受了
              {slots.acceptedAt ? ' · ' + timeOf(slots.acceptedAt) : ''}
            </div>
            <div className="mt-0.5 text-1 text-ink-3" data-kind="task-closed-note">
              任务已关闭；要再改，跟 Kind 说
            </div>
          </div>
        ) : !gateSelf && slots.acceptable ? (
          <>
            <button
              type="button"
              data-kind="task-accept"
              disabled={accepting}
              onClick={accept}
              className="rounded-btn bg-ink px-3 py-1 text-2 text-content transition-opacity hover:opacity-90 disabled:opacity-60"
            >
              接受
            </button>
            <button
              type="button"
              data-kind="task-modify-requirement"
              onClick={() =>
                host.modifyRequirement?.({
                  taskId: slots.taskId,
                  title: slots.title,
                  version: slots.reportVersion,
                })
              }
              className="rounded-btn border border-line-2 px-3 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
            >
              要求修改
            </button>
          </>
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
