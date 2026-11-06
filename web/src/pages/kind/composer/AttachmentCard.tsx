/**
 * 附件卡片（T013.7，F1 起输入框和气泡共用同一张卡）：
 * - 粘贴卡：行数、字数（故事 8），点开看全文；
 * - 文件卡：名字、类型（给了 mime 才显示）、大小（故事 7），点开看全文。
 * 删除按钮只有输入框要（气泡上的卡是发出去的消息，不能删）。
 */

import { FileText, X } from 'lucide-react';
import type { SendAttachmentRef } from '@/data/service';
import { cn } from '@/lib/utils';

/** 文件类型的人话（故事 7：卡片上有类型与大小）。 */
export function typeLabel(mime: string): string {
  if (mime === 'text/plain') return '文字';
  if (mime === 'application/json') return 'JSON';
  if (mime === 'text/markdown') return 'Markdown';
  if (mime === 'text/csv') return 'CSV';
  if (mime.startsWith('text/')) return mime.slice(5) || '文字';
  return mime || '文字';
}

/** 粘贴文本的行数/字数（故事 8：卡片上要显示）。 */
export function pasteStats(text: string): { lines: number; chars: number } {
  return { lines: text.split(String.fromCharCode(10)).length, chars: [...text].length };
}

function sizeLabel(bytes: number): string {
  return Math.max(1, Math.round(bytes / 1024)) + 'KB';
}

interface PasteCardProps {
  paste: string;
  onOpen: () => void;
  onRemove?: () => void;
}

export function PasteCard({ paste, onOpen, onRemove }: PasteCardProps) {
  const stats = pasteStats(paste);
  return (
    <span
      data-kind="paste-card"
      className="flex shrink-0 items-center gap-1 rounded-btn border border-line-2 bg-pane px-2 py-1 text-2 text-ink-2"
    >
      <FileText className="size-3" />
      <button
        type="button"
        data-kind="paste-open"
        onClick={onOpen}
        className="flex min-w-0 items-center gap-1 text-left hover:text-ink"
      >
        <span className="max-w-[160px] truncate">{paste.replace(/\s+/g, ' ').slice(0, 80)}</span>
        <span className="shrink-0 text-ink-3">
          {stats.lines} 行 · {stats.chars} 字
        </span>
      </button>
      {onRemove ? (
        <button type="button" aria-label="移除粘贴" onClick={onRemove} className="text-ink-3 hover:text-ink">
          <X className="size-3" />
        </button>
      ) : null}
    </span>
  );
}

interface FileCardProps {
  file: SendAttachmentRef;
  /** 输入框知道本地文件的 MIME；日志里的文件块只有名字和大小。 */
  mime?: string;
  onOpen: () => void;
  onRemove?: () => void;
}

export function FileCard({ file, mime, onOpen, onRemove }: FileCardProps) {
  return (
    <span
      data-kind="file-card"
      className="flex shrink-0 items-center gap-1 rounded-btn border border-line-2 bg-pane px-2 py-1 text-2 text-ink-2"
    >
      <FileText className="size-3" />
      <button
        type="button"
        data-kind="file-open"
        onClick={onOpen}
        className="flex min-w-0 items-center gap-1 text-left hover:text-ink"
      >
        <span className="max-w-[150px] truncate">{file.name}</span>
        <span className="shrink-0 text-ink-3">
          {mime ? typeLabel(mime) + ' · ' : ''}
          {sizeLabel(file.bytes)}
        </span>
      </button>
      {onRemove ? (
        <button type="button" aria-label="移除文件" onClick={onRemove} className="text-ink-3 hover:text-ink">
          <X className="size-3" />
        </button>
      ) : null}
    </span>
  );
}

interface PreviewProps {
  kind: 'paste' | 'file';
  title: string;
  text: string;
  onClose: () => void;
  /** 输入框里浮在上面；气泡里排在卡片下面（跟着流走）。 */
  className?: string;
}

/** 点开全文的那一块（粘贴和文件共用）。 */
export function AttachmentPreview({ kind, title, text, onClose, className }: PreviewProps) {
  return (
    <div
      data-kind={kind + '-preview'}
      className={cn(
        'max-h-[240px] overflow-auto rounded-card border border-line-2 bg-raised p-2 shadow-[var(--shadow-pop)]',
        className,
      )}
    >
      <div className="mb-1 flex items-center justify-between text-1 text-ink-3">
        <span>{title}</span>
        <button type="button" aria-label={kind === 'paste' ? '关闭粘贴全文' : '关闭文件全文'} onClick={onClose} className="hover:text-ink">
          <X className="size-3" />
        </button>
      </div>
      <pre className="whitespace-pre-wrap break-words font-mono text-2 leading-5 text-ink-2">{text}</pre>
    </div>
  );
}
