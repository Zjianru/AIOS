/**
 * 一格正文（T017.2）：正文槽不超过上限原样显示；超过了先显示前 600 字和「展开全文」，
 * 点开看全文（样子照大段粘贴卡，原文不丢）。
 */

import { useState } from 'react';
import type { BodySlot } from './model';
import { BODY_MAX } from './model';

/** 折起来时显示的前面一段（原文完整保留在 slot.text 里）。 */
export function previewOf(text: string): string {
  return [...text].slice(0, BODY_MAX).join('') + '…';
}

export function BodyText({ slot, className }: { slot: BodySlot; className?: string }) {
  const [open, setOpen] = useState(false);
  if (!slot.text) return null;
  if (!slot.folded || open) {
    return <div className={className}>{slot.text}</div>;
  }
  return (
    <div className={className}>
      <div>{previewOf(slot.text)}</div>
      <button
        type="button"
        data-action="card-expand"
        onClick={() => setOpen(true)}
        className="mt-1 text-1 text-ink-3 transition-colors hover:text-ink"
      >
        展开全文
      </button>
    </div>
  );
}

/** 正文槽放进 <pre> 时用（终端输出、通用卡的输入 / 输出）：超过上限一样先折起来。 */
export function BodyPre({ slot, className }: { slot: BodySlot; className?: string }) {
  const [open, setOpen] = useState(false);
  if (!slot.text) return null;
  const folded = slot.folded && !open;
  return (
    <>
      <pre className={className}>{folded ? previewOf(slot.text) : slot.text}</pre>
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
    </>
  );
}
