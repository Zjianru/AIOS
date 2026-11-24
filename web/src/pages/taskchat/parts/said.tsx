import { useLayoutEffect, useRef, useState } from 'react';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { cn } from '@/lib/utils';

/** D115：任务页/简报共用的「你说的」派生：第一版原话 + 后面各版 + 最近的 Kind 补充。 */
export interface SaidData {
  text: string;
  from: string | null;
  later: { version: number; text: string }[];
  kindNote?: string;
}

export function saidOf(task: TaskReadHeader): SaidData | null {
  const versions = task.requirementVersions ?? [];
  const quoted = versions.flatMap((version) =>
    typeof version.quote === 'string' && version.quote.trim() !== ''
      ? [
          {
            version: version.version,
            quote: version.quote,
            inheritedFrom: version.inheritedFrom ?? null,
          },
        ]
      : [],
  );
  if (quoted.length === 0) return null;
  const first = quoted[0];
  const kindNote = [...versions].reverse().find((version) => version.kindNote)?.kindNote;
  return {
    text: first.quote,
    from: first.inheritedFrom,
    later: quoted.slice(1).map((version) => ({ version: version.version, text: version.quote })),
    ...(kindNote ? { kindNote } : {}),
  };
}

/** 任务页头部的「你说的」：长的露两行，按溢出判断显示展开；子任务标来自总任务。 */
export function Said({ task }: { task: TaskReadHeader }) {
  const [open, setOpen] = useState(false);
  const [clipped, setClipped] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const said = saidOf(task);
  useLayoutEffect(() => {
    const element = box.current;
    if (element && !open) setClipped(element.scrollHeight > element.clientHeight + 1);
  }, [said?.text, said?.later.length, open]);
  if (!said) return null;
  return (
    <div className="mt-3 border-l-2 border-line-2 pl-3" data-kind="task-said">
      <div className="flex items-center gap-1 text-1 text-ink-3">
        <span>你说的</span>
        {said.from ? <span data-kind="task-said-from">· 来自「{said.from}」</span> : null}
      </div>
      <div ref={box} className={cn('mt-0.5 text-3 leading-6 text-ink', !open && 'line-clamp-2')}>
        <div className="whitespace-pre-wrap break-words">{said.text}</div>
        {said.later.map((item) => (
          <div key={item.version} className="mt-1 whitespace-pre-wrap break-words" data-kind="task-said-later">
            <span className="mr-1.5 text-1 text-ink-3">第 {item.version} 版</span>
            {item.text}
          </div>
        ))}
      </div>
      {clipped || open ? (
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          className="mt-0.5 text-1 text-ink-3 transition-colors hover:text-ink-2"
          data-kind="task-said-toggle"
        >
          {open ? '收起' : '展开全文'}
        </button>
      ) : null}
    </div>
  );
}
