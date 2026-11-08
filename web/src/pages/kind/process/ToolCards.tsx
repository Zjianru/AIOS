/**
 * T013.5：工具行展开后的卡片（照原型 app-v14 a5d6f58 的 process/ToolCards.tsx）。
 * 优先顺序：提问 > 终端 > 差异 > 读文件 > 搜索 > 通用的输入/输出。
 * 复制按钮属 T013.6，这里先不放。
 */

import { useState } from 'react';
import { cn } from '@/lib/utils';
import type { AskQuestion } from '@/data/service';
import { diffOf, readOf, searchOf, terminalOf } from './toolModel';
import type { DiffFile, ToolStepView } from './toolModel';

const NL = String.fromCharCode(10);

/** 卡片外壳：代码块的底色、0.5px 边。 */
function Shell({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn('overflow-hidden rounded-info border-[0.5px] border-line-2 bg-pane', className)}>{children}</div>;
}

/* ───────── 终端 ───────── */

function TerminalCard({ step }: { step: ToolStepView }) {
  const card = terminalOf(step)!;
  const failed = card.exitCode !== null && card.exitCode !== 0;
  const pill =
    step.state === 'running'
      ? { text: '运行中', tone: 'text-run' }
      : step.state === 'stopped'
        ? { text: '已停止', tone: 'text-wait' }
        : failed
          ? { text: '退出码 ' + card.exitCode, tone: 'text-fail' }
          : { text: '完成', tone: 'text-ink-3' };
  const dot =
    step.state === 'running' ? 'bg-run animate-pulse' : step.state === 'stopped' ? 'bg-wait' : failed ? 'bg-fail' : 'bg-done';
  return (
    <Shell>
      <div className="flex min-h-8 items-center border-b-[0.5px] border-line-2 pr-1.5">
        <span className="flex w-[30px] shrink-0 justify-center">
          <i className={cn('size-1.5 rounded-full', dot)} />
        </span>
        <code className="min-w-0 flex-1 truncate font-mono text-2 text-ink">{card.command}</code>
        <span className={cn('ml-2 shrink-0 rounded-full bg-hover px-1.5 text-1 leading-5', pill.tone)}>{pill.text}</span>
      </div>
      {card.output ? (
        <pre className="max-h-[224px] overflow-auto whitespace-pre-wrap break-all py-2 pl-[30px] pr-3 font-mono text-2 leading-[18px] text-ink-2">
          {card.output}
        </pre>
      ) : (
        <div className="py-2 pl-[30px] text-2 text-ink-3">{step.state === 'running' ? '等待输出…' : '没有输出'}</div>
      )}
    </Shell>
  );
}

/* ───────── 差异 ───────── */

const DIFF_PREVIEW = 9;

function DiffFileBlock({ file, showPath }: { file: DiffFile; showPath: boolean }) {
  const [all, setAll] = useState(false);
  const lines = all ? file.lines : file.lines.slice(0, DIFF_PREVIEW);
  const hidden = file.lines.length - lines.length;
  return (
    <div>
      {showPath ? (
        <div className="flex items-center gap-2 border-b-[0.5px] border-line-2 px-3 py-1.5 text-2">
          <span className="min-w-0 truncate font-mono text-ink-2">{file.path}</span>
          {file.isNew ? <span className="shrink-0 text-1 text-ink-3">新文件</span> : null}
          <span className="ml-auto shrink-0 font-mono text-1">
            <span className="text-done">+{file.added}</span> <span className="text-fail">-{file.removed}</span>
          </span>
        </div>
      ) : null}
      <div className="overflow-x-auto py-1 font-mono text-2 leading-[18px]">
        {lines.map((line, index) => (
          <div
            key={index}
            className={cn(
              'flex min-w-max',
              line.kind === 'add' && 'bg-done-soft',
              line.kind === 'del' && 'bg-fail-soft',
              line.kind === 'hunk' && 'text-ink-3',
            )}
          >
            <span className="w-10 shrink-0 select-none pr-2 text-right text-ink-3">{line.number ?? ''}</span>
            <span
              className={cn(
                'w-4 shrink-0 select-none',
                line.kind === 'add' && 'text-done',
                line.kind === 'del' && 'text-fail',
              )}
            >
              {line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ''}
            </span>
            <span className={cn('whitespace-pre pr-3', line.kind === 'ctx' ? 'text-ink-2' : 'text-ink')}>{line.text}</span>
          </div>
        ))}
      </div>
      {hidden > 0 ? (
        <button
          type="button"
          onClick={() => setAll(true)}
          className="w-full border-t-[0.5px] border-line-2 py-1 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
        >
          展开其余 {hidden} 行
        </button>
      ) : null}
    </div>
  );
}

function DiffCard({ files }: { files: DiffFile[] }) {
  return (
    <Shell>
      {files.map((file, index) => (
        <div key={file.path + index} className={cn(index > 0 && 'border-t-[0.5px] border-line-2')}>
          <DiffFileBlock file={file} showPath={files.length > 1} />
        </div>
      ))}
    </Shell>
  );
}

/* ───────── 读文件 ───────── */

function ReadCard({ step, cwd }: { step: ToolStepView; cwd: string }) {
  const card = readOf(step, cwd)!;
  return (
    <Shell>
      <div className="max-h-[224px] overflow-auto py-1 font-mono text-2 leading-[18px]">
        {card.lines.map((line) => (
          <div key={line.number} className="flex min-w-max">
            <span className="w-10 shrink-0 select-none pr-3 text-right text-ink-3">{line.number}</span>
            <span className="whitespace-pre pr-3 text-ink-2">{line.text}</span>
          </div>
        ))}
      </div>
      {card.footer ? (
        <div className="border-t-[0.5px] border-line-2 px-3 py-1 text-1 text-ink-3">{card.footer.replace(/^\(|\)$/g, '')}</div>
      ) : null}
    </Shell>
  );
}

/* ───────── 搜索 ───────── */

function SearchCard({ step }: { step: ToolStepView }) {
  const card = searchOf(step)!;
  return (
    <Shell>
      <div className="border-b-[0.5px] border-line-2 px-3 py-1 text-1 text-ink-3">{card.header}</div>
      <div className="max-h-[224px] overflow-auto py-1 font-mono text-2 leading-[18px]">
        {card.groups.map((group) => (
          <div key={group.path} className="px-3 py-0.5">
            <div className="truncate text-ink-2">{group.path}</div>
            {group.rows.map((row) => (
              <div key={row.number} className="flex min-w-max">
                <span className="w-10 shrink-0 select-none pr-3 text-right text-ink-3">{row.number}</span>
                <span className="whitespace-pre text-ink-2">{row.text}</span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </Shell>
  );
}

/* ───────── 提问 ───────── */

function AskCard({ step }: { step: ToolStepView }) {
  const questions = (Array.isArray(step.args.questions) ? step.args.questions : []) as AskQuestion[];
  return (
    <Shell className="px-3 py-2 text-2 leading-5">
      {questions.map((question) => (
        <div key={question.id} className="mb-1">
          {question.header ? <span className="text-ink-3">{question.header} · </span> : null}
          <span className="text-ink">{question.question}</span>
        </div>
      ))}
      {step.output ? (
        <div className="mt-1 text-ink-2">{step.output}</div>
      ) : step.state === 'running' ? (
        <div className="text-ink-3">等你回答…</div>
      ) : null}
    </Shell>
  );
}

/* ───────── 通用：输入 / 输出 ───────── */

function Section({ caption, text, tone }: { caption: string; text: string; tone?: string }) {
  return (
    <div className="px-4 py-3">
      <div className="mb-1 text-1 text-ink-3">{caption}</div>
      <pre className={cn('max-h-[150px] overflow-auto whitespace-pre-wrap break-all font-mono text-2 leading-[18px]', tone ?? 'text-ink-2')}>{text}</pre>
    </div>
  );
}

function GenericCard({ step }: { step: ToolStepView }) {
  const input = JSON.stringify(step.args, null, 2);
  return (
    <Shell>
      <Section caption="输入" text={input} />
      {step.output ? (
        <div className="border-t-[0.5px] border-line-2">
          <Section caption="输出" text={step.output.split(NL).slice(0, 200).join(NL)} tone={step.state === 'error' ? 'text-fail' : undefined} />
        </div>
      ) : null}
    </Shell>
  );
}

/** 这一行有没有可展开的东西（没有就不给点）。 */
export function hasCard(step: ToolStepView): boolean {
  return step.state !== 'running' || step.name === 'bash';
}

export function ToolCard({ step, cwd }: { step: ToolStepView; cwd: string }) {
  if (step.name === 'ask_user_question') return <AskCard step={step} />;
  if (step.name === 'bash') return <TerminalCard step={step} />;
  const diff = diffOf(step, cwd);
  if (diff && diff.length > 0) return <DiffCard files={diff} />;
  if (readOf(step, cwd)) return <ReadCard step={step} cwd={cwd} />;
  if (searchOf(step)) return <SearchCard step={step} />;
  return <GenericCard step={step} />;
}
