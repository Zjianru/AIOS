/**
 * 本票：双击 Esc 的菜单（照 app-v14 c6c7073 的 RewindPanel）。
 *
 * 贴在输入框上面：列表是你发过的话（服务 rewind list，最新在下，默认选最新）；
 * 选一条后四个动作：恢复对话 / 从这里起总结 / 总结到这里为止 / 算了。
 * Esc 在动作里回列表、在列表里关；被拒时顶部红条显示服务端原因。
 */

import { useEffect, useRef, useState } from 'react';
import { CircleAlert, CornerDownLeft } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { TurnViewData } from './viewTypes';
import type { RewindRow } from './rewind.ts';
import { hasBefore, viewOfSeq } from './rewind.ts';

export type RewindAction = 'restore' | 'summarize-from' | 'summarize-up-to';

interface Props {
  phase: 'list' | 'actions';
  rows: RewindRow[];
  views: TurnViewData[];
  selectedSeq: number | null;
  notice: string | null;
  onSelect: (seq: number) => void;
  onBack: () => void;
  onClose: () => void;
  onApply: (action: RewindAction) => void;
}

const ACTIONS: { action: RewindAction | 'cancel'; label: string; hint: string }[] = [
  { action: 'restore', label: '恢复对话', hint: '这句和它后面的都收起，这句放回输入框' },
  { action: 'summarize-from', label: '从这里起总结', hint: '这句到最后压成一段总结，这句放回输入框' },
  { action: 'summarize-up-to', label: '总结到这里为止', hint: '这句之前的压成一段总结，后面的原样留着' },
  { action: 'cancel', label: '算了', hint: '回到列表' },
];

function firstLine(text: string): string {
  const line = text.split('\n').find((row) => row.trim()) ?? '';
  return line.trim() || '（只有附件）';
}

function clockOf(timeMs: number): string {
  const date = new Date(timeMs);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

export function RewindPanel({
  phase,
  rows,
  views,
  selectedSeq,
  notice,
  onSelect,
  onBack,
  onClose,
  onApply,
}: Props) {
  const [listIndex, setListIndex] = useState(0);
  const [actionIndex, setActionIndex] = useState(0);
  const boxRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const index = rows.findIndex((row) => row.seq === selectedSeq);
    setListIndex(index >= 0 ? index : rows.length);
    setActionIndex(0);
    boxRef.current?.focus();
  }, [phase, notice]);

  const selected = rows.find((row) => row.seq === selectedSeq) ?? null;
  const upToBlocked = selected ? !hasBefore(rows, selected.seq) : true;
  const listRows = rows.length + 1;

  const runAction = (index: number) => {
    const item = ACTIONS[index];
    if (!item) return;
    if (item.action === 'cancel') {
      onBack();
      return;
    }
    if (item.action === 'summarize-up-to' && upToBlocked) return;
    onApply(item.action);
  };
  const pickRow = (index: number) => {
    const row = rows[index];
    if (!row) {
      onClose();
      return;
    }
    onSelect(row.seq);
  };

  return (
    <div
      ref={boxRef}
      role="dialog"
      aria-label="回到之前说过的一句"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        const count = phase === 'list' ? listRows : ACTIONS.length;
        const setIndex = phase === 'list' ? setListIndex : setActionIndex;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault();
          const step = event.key === 'ArrowDown' ? 1 : -1;
          setIndex((value) => (value + step + count) % count);
          return;
        }
        if (event.key === 'Enter') {
          event.preventDefault();
          if (phase === 'list') pickRow(listIndex);
          else runAction(actionIndex);
          return;
        }
        if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          if (phase === 'actions') onBack();
          else onClose();
        }
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) onClose();
      }}
      className="absolute inset-x-0 bottom-full z-30 mb-2 overflow-hidden rounded-card border border-line-2 bg-raised py-1 text-2 shadow-[var(--shadow-pop)] outline-none"
      data-kind="rewind-menu"
      data-phase={phase}
    >
      <div className="flex items-center justify-between px-3 pb-1 pt-1.5 text-1 text-ink-3">
        <span>{phase === 'list' ? '回到之前说过的一句' : '这句要怎么办'}</span>
        <span className="flex items-center gap-1">
          ↑↓ 选 · <CornerDownLeft className="size-3" /> 确定 · Esc {phase === 'list' ? '关' : '回列表'}
        </span>
      </div>
      {notice ? (
        <div
          className="mx-2 mb-1 flex items-start gap-1.5 rounded-info bg-fail-soft px-2 py-1.5 text-1 leading-4 text-fail"
          data-kind="rewind-notice"
        >
          <CircleAlert className="mt-px size-3 shrink-0" />
          {notice}
        </div>
      ) : null}

      {phase === 'list' ? (
        <div className="max-h-[320px] overflow-y-auto">
          {rows.length === 0 ? (
            <div className="px-3 py-2 text-ink-3">最后一次压缩之后，你还没说过话。</div>
          ) : null}
          {rows.map((row, index) => {
            const view = viewOfSeq(views, row.seq);
            const extras =
              (view?.user?.files.length ?? 0) +
              (view?.user?.pastes.length ?? 0) +
              (view?.user?.canvas ? 1 : 0) +
              (view?.user?.resultCard ? 1 : 0);
            return (
              <div
                key={row.seq}
                role="option"
                aria-selected={index === listIndex}
                onMouseEnter={() => setListIndex(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => pickRow(index)}
                className={cn(
                  'flex cursor-default items-baseline gap-3 px-3 py-1.5',
                  index === listIndex && 'bg-hover',
                )}
                data-kind="rewind-row"
              >
                <span className="w-[64px] shrink-0 text-1 tabular-nums text-ink-3">
                  {clockOf(row.time)}
                </span>
                <span className="min-w-0 flex-1 truncate text-ink">{firstLine(row.text)}</span>
                {extras > 0 ? (
                  <span className="shrink-0 text-1 text-ink-3">带 {extras} 样</span>
                ) : null}
              </div>
            );
          })}
          <div
            role="option"
            aria-selected={listIndex === rows.length}
            onMouseEnter={() => setListIndex(rows.length)}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => onClose()}
            className={cn(
              'cursor-default px-3 py-1.5 text-ink-2',
              listIndex === rows.length && 'bg-hover',
            )}
            data-kind="rewind-cancel"
          >
            算了
          </div>
        </div>
      ) : selected ? (
        <div>
          <div className="mx-3 mb-1 truncate border-l-2 border-line-2 pl-2 text-ink-2">
            <span className="mr-2 text-1 text-ink-3">{clockOf(selected.time)}</span>
            {firstLine(selected.text)}
          </div>
          {ACTIONS.map((item, index) => {
            const blocked = item.action === 'summarize-up-to' && upToBlocked;
            return (
              <div
                key={item.action}
                role="option"
                aria-selected={index === actionIndex}
                aria-disabled={blocked || undefined}
                onMouseEnter={() => setActionIndex(index)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => runAction(index)}
                className={cn(
                  'flex cursor-default items-baseline gap-3 px-3 py-1.5',
                  index === actionIndex && 'bg-hover',
                  blocked && 'opacity-50',
                )}
                data-kind="rewind-action"
                data-action={item.action}
              >
                <span className="w-[112px] shrink-0 text-ink">{item.label}</span>
                <span className="min-w-0 truncate text-1 text-ink-3">
                  {blocked ? '这句前面没有可以总结的' : item.hint}
                </span>
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
