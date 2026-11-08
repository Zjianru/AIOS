/**
 * 工具卡的视图：终端、差异、读文件、搜索。
 *
 * - 槽位已经由卡片入口（<Card>）按每类卡的 schema 校验过，这里不再回头去看工具调用的原始形状；
 * - 样子照既有工具卡（D61：外观、行为不变；正文槽超长时多一个「展开全文」）；
 * - 终端的「退出码 N / 完成」仍按实际退出码算，信封的 state 只用来区分运行中 / 已停止。
 */

import { useState } from 'react';
import type { ReactNode } from 'react';
import { cn } from '@/lib/utils';
import type {
  CardViewProps,
  DiffFileSlot,
  DiffSlots,
  ReadSlots,
  SearchSlots,
  TerminalSlots,
} from './model';
import { BodyPre } from './BodyText';

/** 卡片外壳：代码块的底色、0.5px 边。 */
export function Shell({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('overflow-hidden rounded-info border-[0.5px] border-line-2 bg-pane', className)}>{children}</div>;
}

/* ───────── 终端 ───────── */

export function TerminalCardView({ envelope, slots }: CardViewProps<TerminalSlots>) {
  const failed = slots.exitCode !== null && slots.exitCode !== 0;
  const pill =
    envelope.state === 'running'
      ? { text: '运行中', tone: 'text-run' }
      : envelope.state === 'stopped'
        ? { text: '已停止', tone: 'text-wait' }
        : failed
          ? { text: '退出码 ' + String(slots.exitCode), tone: 'text-fail' }
          : { text: '完成', tone: 'text-ink-3' };
  const dot =
    envelope.state === 'running'
      ? 'bg-run animate-pulse'
      : envelope.state === 'stopped'
        ? 'bg-wait'
        : failed
          ? 'bg-fail'
          : 'bg-done';
  return (
    <Shell>
      <div className="flex min-h-8 items-center border-b-[0.5px] border-line-2 pr-1.5">
        <span className="flex w-[30px] shrink-0 justify-center">
          <i className={cn('size-1.5 rounded-full', dot)} />
        </span>
        <code className="min-w-0 flex-1 truncate font-mono text-2 text-ink">{slots.command}</code>
        <span className={cn('ml-2 shrink-0 rounded-full bg-hover px-1.5 text-1 leading-5', pill.tone)}>{pill.text}</span>
      </div>
      {slots.output.text ? (
        <BodyPre
          slot={slots.output}
          className="max-h-[224px] overflow-auto whitespace-pre-wrap break-all py-2 pl-[30px] pr-3 font-mono text-2 leading-[18px] text-ink-2"
        />
      ) : (
        <div className="py-2 pl-[30px] text-2 text-ink-3">{envelope.state === 'running' ? '等待输出…' : '没有输出'}</div>
      )}
    </Shell>
  );
}

/* ───────── 差异 ───────── */

const DIFF_PREVIEW = 9;

function DiffFileBlock({ file, showPath }: { file: DiffFileSlot; showPath: boolean }) {
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

export function DiffCardView({ slots }: CardViewProps<DiffSlots>) {
  if (slots.files.length === 0) return null;
  return (
    <Shell>
      {slots.files.map((file, index) => (
        <div key={file.path + String(index)} className={cn(index > 0 && 'border-t-[0.5px] border-line-2')}>
          <DiffFileBlock file={file} showPath={slots.files.length > 1} />
        </div>
      ))}
    </Shell>
  );
}

/* ───────── 读文件 ───────── */

export function ReadCardView({ slots }: CardViewProps<ReadSlots>) {
  return (
    <Shell>
      <div className="max-h-[224px] overflow-auto py-1 font-mono text-2 leading-[18px]">
        {slots.lines.map((line) => (
          <div key={line.number} className="flex min-w-max">
            <span className="w-10 shrink-0 select-none pr-3 text-right text-ink-3">{line.number}</span>
            <span className="whitespace-pre pr-3 text-ink-2">{line.text}</span>
          </div>
        ))}
      </div>
      {slots.footer ? (
        <div className="border-t-[0.5px] border-line-2 px-3 py-1 text-1 text-ink-3">{slots.footer.replace(/^\(|\)$/g, '')}</div>
      ) : null}
    </Shell>
  );
}

/* ───────── 搜索 ───────── */

export function SearchCardView({ slots }: CardViewProps<SearchSlots>) {
  return (
    <Shell>
      <div className="border-b-[0.5px] border-line-2 px-3 py-1 text-1 text-ink-3">{slots.header}</div>
      <div className="max-h-[224px] overflow-auto py-1 font-mono text-2 leading-[18px]">
        {slots.groups.map((group, index) => (
          <div key={group.path + String(index)} className="px-3 py-0.5">
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
