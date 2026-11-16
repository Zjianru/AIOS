//! 账本：任务账本条目的分组与时间。

import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import type { TaskReadLedger } from '@/data/generated/TaskReadLedger';
import { cn } from '@/lib/utils';
import { clock } from './helpers';

export const NOTE_GROUPS = [
  { kind: 'next', label: '下一步' },
  { kind: 'rejected', label: '否掉的做法' },
] as const;


/**
 * ：任务页账本（照原型 app-v14 d2a28f2）。
 * 约束是用户原话，进展/待答从最近报告读；叙述条按类分组；关掉的历史点开才看。
 */
export function Ledger({ ledger, task }: { ledger: TaskReadLedger; task: TaskReadHeader }) {
  const [open, setOpen] = useState(false);
  const [history, setHistory] = useState(false);
  const constraints = ledger.constraints;
  const live = ledger.notes;
  const closed = [...ledger.history].sort((left, right) => right.closedAt - left.closedAt);
  const report = task.report;
  if (constraints.length === 0 && live.length === 0 && closed.length === 0 && !report) {
    return null;
  }
  const count = constraints.length + live.length + (report ? 1 : 0);
  return (
    <div className="mt-1" data-kind="task-ledger">
      <button
        type="button"
        data-kind="task-ledger-toggle"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
        账本 · {count} 条在用
      </button>
      {open ? (
        <div
          className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-l border-line pl-3 text-2 leading-5"
          data-kind="task-ledger-body"
        >
          {constraints.length ? (
            <>
              <span className="text-ink-3">约束</span>
              <div className="min-w-0 space-y-0.5" data-kind="ledger-constraints">
                {constraints.map((item) => (
                  <div key={item.text} className="text-ink-2">
                    「{item.text}」
                    <span className="text-ink-3">
                      {item.from ? ' · 来自「' + item.from + '」' : ' · 你说的'}
                    </span>
                  </div>
                ))}
              </div>
            </>
          ) : null}
          {report ? (
            <>
              <span className="text-ink-3">进展</span>
              <div className="min-w-0 text-ink-2" data-kind="ledger-progress">
                <span className="line-clamp-2">{report.summary}</span>
                <span className="text-ink-3">
                  第 {Math.max(1, report.attempt)} 次
                  {task.reportedAt != null ? ' · ' + clock(task.reportedAt) + ' 的报告' : ''}
                </span>
              </div>
            </>
          ) : null}
          {report?.question ? (
            <>
              <span className="text-ink-3">待答</span>
              <div className="min-w-0 text-ink-2" data-kind="ledger-question">
                {report.question}
              </div>
            </>
          ) : null}
          {NOTE_GROUPS.map((group) => {
            const notes = live.filter((note) => note.kind === group.kind);
            if (notes.length === 0) return null;
            return (
              <div key={group.kind} className="contents">
                <span className="text-ink-3">{group.label}</span>
                <div className="min-w-0 space-y-0.5" data-kind={'ledger-' + group.kind}>
                  {notes.map((note) => (
                    <div key={note.id} className="text-ink-2">
                      {note.text}
                      <span className="text-ink-3"> · 第 {note.attempt} 次记</span>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
          {closed.length ? (
            <>
              <span />
              <div className="min-w-0">
                <button
                  type="button"
                  onClick={() => setHistory((value) => !value)}
                  className="text-1 text-ink-3 transition-colors hover:text-ink-2"
                  data-kind="ledger-history-toggle"
                >
                  {history ? '收起历史' : '历史 ' + closed.length + ' 条（不再带给它）'}
                </button>
                {history ? (
                  <div className="mt-1 space-y-1" data-kind="ledger-history">
                    {closed.map((note) => (
                      <div key={note.id} className="text-1 leading-4 text-ink-3">
                        <span className="text-ink-2 line-through decoration-ink-4">{note.text}</span>
                        <br />
                        {NOTE_GROUPS.find((group) => group.kind === note.kind)?.label} · 第{' '}
                        {note.attempt} 次记 · {note.closedWhy}，{clock(note.closedAt)}
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
