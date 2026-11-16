//! 程序检查：门/检查行/运行状态。

import { useState } from 'react';
import { Terminal } from 'lucide-react';
import type { TaskReadGate } from '@/data/generated/TaskReadGate';
import type { TaskReadGateCheck } from '@/data/generated/TaskReadGateCheck';
import { cn } from '@/lib/utils';
import { clock } from './helpers';

export const GATE_OUTCOME: Record<string, { text: string; tone: string }> = {
  pass: { text: '通过', tone: 'text-done' },
  needs_fix: { text: '需修正', tone: 'text-wait' },
  undetermined: { text: '未确定', tone: 'text-ink-2' },
};


export function programCheckWord(check: TaskReadGateCheck): { text: string; tone: string } {
  if (check.outcome === 'timed_out') return { text: '超时', tone: 'text-wait' };
  if (check.outcome === 'spawn_failed') return { text: '起不来', tone: 'text-fail' };
  if (check.outcome === 'stopped') return { text: '被停', tone: 'text-ink-3' };
  if (check.outcome === 'skipped') return { text: '没跑', tone: 'text-ink-3' };
  return check.exitCode === 0
    ? { text: '退出码 0', tone: 'text-done' }
    : { text: '退出码 ' + String(check.exitCode ?? '?'), tone: 'text-fail' };
}


export function programDuration(ms: number): string {
  return ms >= 1000 ? (ms / 1000).toFixed(ms >= 10_000 ? 0 : 1) + ' 秒' : ms + ' 毫秒';
}


export function ProgramCheckRows({ checks }: { checks: TaskReadGateCheck[] }) {
  return (
    <div className="mt-2 space-y-1.5" data-kind="program-checks">
      {checks.map((check) => {
        const word = programCheckWord(check);
        const bad =
          check.outcome === 'timed_out' ||
          check.outcome === 'spawn_failed' ||
          (check.exitCode !== undefined && check.exitCode !== null && check.exitCode !== 0);
        return (
          <div key={check.index} data-kind="program-check" data-exit={check.exitCode ?? 'none'}>
            <div className="flex min-w-0 items-center gap-2">
              <Terminal className="size-3 shrink-0 text-ink-3" />
              <code className="process-path min-w-0 flex-1 truncate text-ink-2" title={check.command}>
                {check.command}
              </code>
              <span className={cn('shrink-0 text-1', word.tone)}>{word.text}</span>
              {check.durationMs > 0 ? (
                <span className="brand-type shrink-0 text-1 text-ink-3">
                  {programDuration(check.durationMs)}
                </span>
              ) : null}
            </div>
            {bad && check.tail ? (
              <pre className="mt-1 max-h-[120px] overflow-auto whitespace-pre-wrap break-words rounded-btn bg-hover/60 px-2 py-1 font-mono text-1 leading-4 text-ink-2" data-kind="program-tail">
                {check.tail}
              </pre>
            ) : null}
            {bad && check.spill ? (
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


export function ProgramSection({
  gate,
  running,
  index,
}: {
  gate: TaskReadGate;
  running: boolean;
  index: number | null;
}) {
  const [older, setOlder] = useState(false);
  const runs = gate.runs ?? [];
  const shown = older ? runs : runs.slice(0, 1);
  return (
    <div className="mt-3 border-t border-line pt-3" data-kind="program-section">
      <div className="text-2 font-medium text-ink">程序检查</div>
      <div className="mt-0.5 text-1 leading-4 text-ink-3">
        每交来一份结果，程序在任务目录里按顺序跑下面的命令；全部退出码为 0 就通过。命令是你在确认卡上允许的第{' '}
        {gate.commandsVersion} 版。
      </div>
      <ol className="mt-2 space-y-1" data-kind="program-commands">
        {gate.commands.map((command, commandIndex) => (
          <li key={commandIndex} className="flex min-w-0 items-center gap-2 text-2 leading-5">
            <span className="brand-type w-4 shrink-0 text-right text-1 text-ink-3">
              {commandIndex + 1}
            </span>
            <code className="process-path min-w-0 flex-1 truncate text-ink">{command.command}</code>
            {running && index === commandIndex ? (
              <span className="shrink-0 text-1 text-run">在跑</span>
            ) : running && index !== null && index < commandIndex ? (
              <span className="shrink-0 text-1 text-ink-3">排着</span>
            ) : null}
          </li>
        ))}
      </ol>
      {running ? (
        <div className="mt-3 flex items-center gap-1.5 text-2 text-run" data-kind="program-running">
          <Terminal className="size-3.5" />
          第 {(index ?? 0) + 1} 条命令在跑
        </div>
      ) : runs.length === 0 ? (
        <div className="mt-3 text-2 text-ink-3" data-kind="program-empty">
          还没跑过：等被把关任务交来结果。
        </div>
      ) : null}
      {shown.map((run) => (
        <div key={run.attempt} className="mt-4 border-t border-line pt-3" data-kind="program-run" data-outcome={run.conclusion}>
          <div className="flex flex-wrap items-baseline gap-x-2 text-2">
            <span className="text-ink">第 {run.attempt} 次检查</span>
            <span className={cn('font-medium', GATE_OUTCOME[run.conclusion]?.tone ?? 'text-ink-2')}>
              {GATE_OUTCOME[run.conclusion]?.text ?? run.conclusion}
            </span>
            <span className="text-1 text-ink-3">
              第 {run.commandsVersion} 版命令 · {clock(run.at)}
            </span>
          </div>
          <ProgramCheckRows checks={run.checks} />
          {run.conclusion === 'undetermined' ? (
            <div className="mt-1.5 text-1 text-ink-3">
              未确定不算通过，也不算有错；已交给 Kind，重跑还是换命令由你定。
            </div>
          ) : null}
        </div>
      ))}
      {runs.length > 1 ? (
        <button
          type="button"
          onClick={() => setOlder((value) => !value)}
          className="mt-3 text-1 text-ink-3 transition-colors hover:text-ink-2"
          data-kind="program-older"
        >
          {older ? '只看最新一次' : '更早的 ' + String(runs.length - 1) + ' 次：' + String(runs.slice(1).map((run) => GATE_OUTCOME[run.conclusion]?.text ?? run.conclusion).join('、'))}
        </button>
      ) : null}
    </div>
  );
}


/**
 * 页头状态：顺序照 spec——在跑 → 执行中（收尾时「执行中 · 收尾」）；已停止；被打断了；
 * 有 report → taskStatusOf(report.state, report.origin, review, accepted)；其余 → 等待中。
 * 词只在 taskStatusOf 一处；旧的笼统说法（交过报告就写待核）不再用。
 */
export function ProgramState({
  running,
  gate,
}: {
  running: boolean;
  gate: TaskReadGate;
}) {
  const word = running
    ? '程序在跑'
    : gate.conclusion === 'pass'
      ? '程序判了通过'
      : gate.conclusion === 'needs_fix'
        ? '程序判了需修正'
        : gate.conclusion === 'undetermined'
          ? '未确定'
          : '等待中';
  const tone = running
    ? 'bg-run-soft text-run'
    : gate.conclusion === 'pass'
      ? 'bg-done-soft text-done'
      : gate.conclusion === 'needs_fix'
        ? 'bg-wait-soft text-wait'
        : 'bg-hover text-ink-2';
  return (
    <span
      className={cn(
        'mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge px-1.5 py-[1px] text-1',
        tone,
      )}
      data-kind="program-state"
    >
      <Terminal className="size-3 shrink-0" />
      <span className="truncate">{word}</span>
    </span>
  );
}
