/** 记忆小卡：一行高、书签、浅底虚线框；状态按 ref 从记忆库读，按钮由程序定。 */
import { Bookmark } from 'lucide-react';
import { toast } from 'sonner';
import type { MemoryActionKind } from '@/data/service';
import { go } from '@/app/navigate';
import { performMemoryAction, useMemoryCardStates } from './memoryCards';

export type MemoCardTarget =
  | { kind: 'memory' | 'diary'; id: string }
  | { kind: 'conversation'; turn: number; seq: number };

export interface AboutEditMemo {
  op: 'add' | 'replace' | 'delete';
  aboutId?: string;
  old?: string;
  text?: string;
}

export interface MemoCard {
  kind: string;
  id?: string;
  title?: string;
  /** 状态从记忆库读时用的引用；程序卡只有 ref，工具卡用记忆 id。 */
  ref?: string;
  /** 搜索卡的出处目标：点「看出处」能落到条目或那一轮。 */
  target?: MemoCardTarget;
  /** 替换卡：旧/新条目和文案。 */
  oldId?: string;
  newId?: string;
  oldTitle?: string;
  oldText?: string;
  newText?: string;
  /** 「关于你」草稿卡。 */
  quote?: string;
  edits?: AboutEditMemo[];
}

function wordOf(card: MemoCard, state: string | undefined): string {
  const kind = card.kind;
  if (kind === 'replace') {
    if (state === 'replaced') return '替换了「' + (card.oldTitle ?? '') + '」· 旧的留在修订里';
    if (state === 'kept') return '两条都留 · 「' + (card.oldTitle ?? '') + '」照旧';
    return '这条会替换你定的「' + (card.oldTitle ?? '') + '」';
  }
  if (kind === 'about_draft') {
    const count = card.edits?.length ?? 0;
    if (state === 'saved') return '「关于你」改了 ' + count + ' 处 · 记忆页能看修订';
    if (state === 'dropped') return '没用这份草稿 · 「关于你」没动';
    return 'Kind 起草了「关于你」的 ' + count + ' 处改动 · 依据你说的';
  }
  if (kind === 'permission') return state === 'accepted' ? '已允许' : '做梦许可';
  if (kind === 'dream_done') return state === 'revoked' ? '整理完了 · 已撤销' : '整理完了';
  if (kind === 'dream_failed') return '整理没跑成';
  if (kind === 'remember') {
    // 撤销走隔离区时 card_state 是 deleted；两种终态都显示已撤销。
    if (state === 'revoked' || state === 'deleted') return '记下了 · 已撤销';
    return '记下了';
  }
  if (kind === 'note') {
    if (state === 'deleted') return 'Kind 记了一笔 · 已删';
    if (state === 'revoked') return 'Kind 记了一笔 · 已撤销';
    return 'Kind 记了一笔';
  }
  if (kind === 'search') return '翻了记忆';
  return '记下了';
}

const linkClass =
  'shrink-0 text-2 text-ink-3 underline-offset-2 hover:text-ink hover:underline';
const primaryLinkClass = 'shrink-0 text-2 text-brand underline-offset-2 hover:underline';

export function MemoChip({ card }: { card: MemoCard }) {
  const reference = card.ref ?? card.id ?? '';
  const states = useMemoryCardStates([reference]);
  const state = states[reference]?.state;
  const reason = states[reference]?.error;
  const word = wordOf(card, state);

  const act = (action: MemoryActionKind) => {
    void performMemoryAction(action)
      .then(({ message }) => {
        if (message) toast(message);
      })
      .catch((error: unknown) => {
        toast(error instanceof Error ? error.message : '没成功');
      });
  };

  const actions: Array<{ label: string; action: string; primary?: boolean; run: () => void }> = [];
  if (card.kind === 'permission' && state !== 'accepted' && reference) {
    actions.push({
      label: '允许',
      action: 'memory-permit-allow',
      primary: true,
      run: () => act({ kind: 'set-permit', allow: true }),
    });
    actions.push({
      label: '暂不',
      action: 'memory-permit-later',
      run: () => act({ kind: 'set-permit', allow: false }),
    });
  } else if (card.kind === 'dream_done' && reference) {
    if (state !== 'revoked') {
      actions.push({
        label: '看看',
        action: 'memory-dream-see',
        run: () => {
          // 带上这次 run 的 id：记忆页要定位到这一行的明细，而不是最新一行。
          sessionStorage.setItem('aios:memory-run', reference);
          go('memory');
        },
      });
      actions.push({
        label: '全部撤销',
        action: 'memory-dream-undo',
        primary: true,
        run: () => act({ kind: 'undo-dream', runId: reference }),
      });
    }
  } else if (card.kind === 'dream_failed') {
    actions.push({ label: '看看', action: 'memory-dream-see', run: () => go('memory') });
  } else if (card.kind === 'remember' && card.id) {
    if (state !== 'revoked' && state !== 'deleted') {
      actions.push({ label: '改', action: 'memory-edit', run: () => go('memory') });
      // 小卡上的撤销只进隔离区（7 天内能恢复）；彻底删只在记忆页。
      actions.push({
        label: '撤销',
        action: 'memory-delete',
        run: () => act({ kind: 'delete', id: card.id! }),
      });
    }
  } else if (card.kind === 'note' && card.id) {
    if (state !== 'deleted' && state !== 'revoked') {
      actions.push({ label: '看', action: 'memory-see', run: () => go('memory') });
      actions.push({
        label: '删',
        action: 'memory-delete',
        run: () => act({ kind: 'delete', id: card.id! }),
      });
    }
  } else if (card.kind === 'replace' && reference) {
    if (state !== 'replaced' && state !== 'kept') {
      actions.push({
        label: '替换',
        action: 'memory-replace',
        primary: true,
        run: () => act({ kind: 'resolve-replace', card: reference, replace: true }),
      });
      actions.push({
        label: '两条都留',
        action: 'memory-keep',
        run: () => act({ kind: 'resolve-replace', card: reference, replace: false }),
      });
    }
  } else if (card.kind === 'about_draft' && reference) {
    if (state !== 'saved' && state !== 'dropped') {
      actions.push({
        label: '保存',
        action: 'memory-about-save',
        primary: true,
        run: () => act({ kind: 'resolve-about-draft', card: reference, save: true }),
      });
      actions.push({
        label: '不用',
        action: 'memory-about-drop',
        run: () => act({ kind: 'resolve-about-draft', card: reference, save: false }),
      });
    }
  } else if (card.kind === 'search') {
    actions.push({
      label: '看出处',
      action: 'memory-recall-source',
      run: () => {
        if (!card.target) {
          go('memory');
          return;
        }
        if (card.target.kind === 'conversation') {
          sessionStorage.setItem(
            'aios:memory-jump',
            JSON.stringify({ turn: card.target.turn, seq: card.target.seq }),
          );
          go('kind');
          return;
        }
        sessionStorage.setItem('aios:memory-highlight', card.target.id);
        go('memory');
      },
    });
  }

  return (
    <div
      data-kind="memo-chip"
      data-memo-kind={card.kind}
      data-ref={reference}
      data-state={state ?? ''}
      className="mt-1 inline-flex max-w-full items-center gap-1.5 rounded-btn border border-dashed border-line-2 bg-hover/60 px-2 py-0.5 text-1 leading-4 text-ink-3"
    >
      <Bookmark className="size-3 shrink-0 text-wait" strokeWidth={1.7} />
      <span className="shrink-0">{word}</span>
      {card.title && card.kind !== 'about_draft' ? (
        <span className="min-w-0 truncate text-ink-2">{card.title}</span>
      ) : null}
      {card.kind === 'replace' && card.oldText !== undefined && card.newText !== undefined ? (
        <span className="min-w-0 truncate" data-kind="memo-replace-diff">
          <span className="line-through decoration-ink-3/60">{card.oldText}</span>
          <span className="px-1">→</span>
          <span className="text-ink-2">{card.newText}</span>
        </span>
      ) : null}
      {card.kind === 'about_draft' && card.quote ? (
        <span className="min-w-0 truncate text-ink-3" data-kind="memo-about-quote">
          「{card.quote}」
        </span>
      ) : null}
      {card.kind === 'dream_failed' && reason ? (
        <span
          data-kind="memo-error"
          className="min-w-0 max-w-[24rem] truncate text-ink-3"
          title={reason}
        >
          {reason}
        </span>
      ) : null}
      {card.kind === 'about_draft' && card.edits && card.edits.length > 0 ? (
        <span className="flex min-w-0 flex-col gap-0.5" data-kind="memo-about-edits">
          {card.edits.map((edit, index) => (
            <span key={index} className="min-w-0 truncate">
              <span className="pr-1 text-ink-3">
                {edit.op === 'add' ? '加' : edit.op === 'replace' ? '改' : '删'}
              </span>
              {edit.op === 'add' ? <span className="text-ink-2">{edit.text}</span> : null}
              {edit.op === 'replace' ? (
                <>
                  <span className="line-through decoration-ink-3/60">{edit.old}</span>
                  <span className="px-1">→</span>
                  <span className="text-ink-2">{edit.text}</span>
                </>
              ) : null}
              {edit.op === 'delete' ? (
                <span className="line-through decoration-ink-3/60">{edit.old}</span>
              ) : null}
            </span>
          ))}
        </span>
      ) : null}
      {actions.map((item) => (
        <button
          key={item.action}
          type="button"
          data-action={item.action}
          onClick={(event) => {
            event.stopPropagation();
            item.run();
          }}
          className={item.primary ? primaryLinkClass : linkClass}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

export function memoryCardOf(meta: Record<string, unknown> | null | undefined): MemoCard | null {
  if (!meta || typeof meta !== 'object') return null;
  const value = (meta as { memoryCard?: unknown }).memoryCard;
  if (!value || typeof value !== 'object') return null;
  const card = value as {
    kind?: unknown;
    id?: unknown;
    title?: unknown;
    ref?: unknown;
    target?: unknown;
    oldId?: unknown;
    newId?: unknown;
    oldTitle?: unknown;
    oldText?: unknown;
    newText?: unknown;
    quote?: unknown;
    edits?: unknown;
  };
  if (typeof card.kind !== 'string') return null;
  const id = typeof card.id === 'string' ? card.id : undefined;
  return {
    kind: card.kind,
    id,
    title: typeof card.title === 'string' ? card.title : undefined,
    ref: typeof card.ref === 'string' ? card.ref : id,
    target: parseTarget(card.target),
    oldId: typeof card.oldId === 'string' ? card.oldId : undefined,
    newId: typeof card.newId === 'string' ? card.newId : undefined,
    oldTitle: typeof card.oldTitle === 'string' ? card.oldTitle : undefined,
    oldText: typeof card.oldText === 'string' ? card.oldText : undefined,
    newText: typeof card.newText === 'string' ? card.newText : undefined,
    quote: typeof card.quote === 'string' ? card.quote : undefined,
    edits: parseEdits(card.edits),
  };
}

function parseEdits(value: unknown): AboutEditMemo[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const edits: AboutEditMemo[] = [];
  for (const item of value) {
    if (!item || typeof item !== 'object') continue;
    const edit = item as { op?: unknown; about_id?: unknown; old?: unknown; text?: unknown };
    const op = edit.op;
    if (op !== 'add' && op !== 'replace' && op !== 'delete') continue;
    edits.push({
      op,
      aboutId: typeof edit.about_id === 'string' ? edit.about_id : undefined,
      old: typeof edit.old === 'string' ? edit.old : undefined,
      text: typeof edit.text === 'string' ? edit.text : undefined,
    });
  }
  return edits;
}

function parseTarget(value: unknown): MemoCardTarget | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const target = value as { kind?: unknown; id?: unknown; turn?: unknown; seq?: unknown };
  if ((target.kind === 'memory' || target.kind === 'diary') && typeof target.id === 'string') {
    return { kind: target.kind, id: target.id };
  }
  if (
    target.kind === 'conversation' &&
    typeof target.turn === 'number' &&
    typeof target.seq === 'number'
  ) {
    return { kind: 'conversation', turn: target.turn, seq: target.seq };
  }
  return undefined;
}
