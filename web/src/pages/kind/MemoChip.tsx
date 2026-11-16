/** 记忆小卡：一行高、书签、浅底虚线框；只显示已有事实，不做写操作。 */
import { Bookmark } from 'lucide-react';

export interface MemoCard {
  kind: string;
  id?: string;
  title?: string;
  /** 状态从记忆库读时用的引用；程序卡只有 ref，工具卡用记忆 id。 */
  ref?: string;
}

export function MemoChip({ card }: { card: MemoCard }) {
  const word =
    card.kind === 'remember'
      ? '记下了'
      : card.kind === 'note'
        ? 'Kind 记了一笔'
        : card.kind === 'search'
          ? '翻了记忆'
          : card.kind === 'permission'
            ? '做梦许可'
            : card.kind === 'dream_done'
              ? '整理完了'
              : '整理没跑成';
  return (
    <div
      data-kind="memo-chip"
      data-memo-kind={card.kind}
      data-ref={card.ref ?? card.id ?? ''}
      className="mt-1 inline-flex max-w-full items-center gap-1.5 rounded-btn border border-dashed border-line-2 bg-hover/60 px-2 py-0.5 text-1 leading-4 text-ink-3"
    >
      <Bookmark className="size-3 shrink-0 text-wait" strokeWidth={1.7} />
      <span className="shrink-0">{word}</span>
      {card.title ? <span className="min-w-0 truncate text-ink-2">{card.title}</span> : null}
    </div>
  );
}

export function memoryCardOf(meta: Record<string, unknown> | null | undefined): MemoCard | null {
  if (!meta || typeof meta !== 'object') return null;
  const value = (meta as { memoryCard?: unknown }).memoryCard;
  if (!value || typeof value !== 'object') return null;
  const card = value as { kind?: unknown; id?: unknown; title?: unknown; ref?: unknown };
  if (typeof card.kind !== 'string') return null;
  const id = typeof card.id === 'string' ? card.id : undefined;
  return {
    kind: card.kind,
    id,
    title: typeof card.title === 'string' ? card.title : undefined,
    ref: typeof card.ref === 'string' ? card.ref : id,
  };
}
