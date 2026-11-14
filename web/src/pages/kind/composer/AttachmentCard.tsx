/**
 * 附件卡片（T013.7，F1 起输入框和气泡共用同一张卡）：
 * - 粘贴卡：行数、字数（故事 8），点开看全文；
 * - 文件卡：名字、类型（从后缀或 mime 推）、大小（故事 7），点开看全文。
 * 删除按钮只有输入框要（气泡上的卡是发出去的消息，不能删）。
 */

import { AppWindow, FileText, Quote, X } from 'lucide-react';
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

/** 文件名后缀 → 类型的人话（回放的 file 块没有 mime，只能从名字推）。 */
const EXTENSION_TYPES: Record<string, string> = {
  txt: '文字',
  text: '文字',
  log: '日志',
  md: 'Markdown',
  markdown: 'Markdown',
  json: 'JSON',
  jsonl: 'JSONL',
  ndjson: 'JSONL',
  csv: 'CSV',
  tsv: 'TSV',
  yaml: 'YAML',
  yml: 'YAML',
  toml: 'TOML',
  xml: 'XML',
  html: 'HTML',
  css: 'CSS',
  js: 'JavaScript',
  jsx: 'JavaScript',
  ts: 'TypeScript',
  tsx: 'TypeScript',
  rs: 'Rust',
  py: 'Python',
  sh: 'Shell',
  bash: 'Shell',
  zsh: 'Shell',
  sql: 'SQL',
  go: 'Go',
  java: 'Java',
  c: 'C',
  h: 'C',
  cpp: 'C++',
  hpp: 'C++',
  diff: 'Diff',
};

/**
 * 文件卡上的类型：先看文件名后缀（气泡回放的 file 块只有名字和大小，没有 mime），
 * 后缀不认识才用浏览器给的 mime；都没有就当文字。
 */
export function fileTypeLabel(name: string, mime?: string): string {
  const dot = name.lastIndexOf('.');
  const extension = dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
  const byExtension = EXTENSION_TYPES[extension];
  if (byExtension) return byExtension;
  if (mime) return typeLabel(mime);
  return '文字';
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

interface QuoteCardProps {
  quote: string;
  onRemove: () => void;
}

/** 输入框上方的引用卡（G3）：前 80 字 + 行数，和粘贴/文件卡同一套卡片外观。 */
export function QuoteCard({ quote, onRemove }: QuoteCardProps) {
  const lines = quote.split(String.fromCharCode(10)).length;
  return (
    <span
      data-kind="quote-card"
      className="flex shrink-0 items-center gap-1 rounded-btn border border-line-2 bg-pane px-2 py-1 text-2 text-ink-2"
    >
      <Quote className="size-3" />
      <span className="max-w-[160px] truncate" title={quote}>
        {quote.replace(/\s+/g, ' ').slice(0, 80)}
      </span>
      <span className="shrink-0 text-ink-3">{lines} 行</span>
      <button type="button" aria-label="移除引用" onClick={onRemove} className="text-ink-3 hover:text-ink">
        <X className="size-3" />
      </button>
    </span>
  );
}

interface FileCardProps {
  file: SendAttachmentRef;
  /** 输入框知道本地文件的 MIME；日志里的文件块只有名字和大小（回放从后缀推）。 */
  mime?: string;
  onOpen: () => void;
  onRemove?: () => void;
}

export function FileCard({ file, mime, onOpen, onRemove }: FileCardProps) {
  const type = fileTypeLabel(file.name, mime);
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
          {type} · {sizeLabel(file.bytes)}
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

interface FromCanvasChipProps {
  title: string;
  /** 输入框里可以移除（当成你自己写的）；气泡上的标记不给删除。 */
  onRemove?: () => void;
  className?: string;
}

/**
 * 「来自画布」（T015.4）：画布里的按钮放进输入框的那一句话带着它（D58）；
 * 发出去以后气泡上也留着；输入框里可以移除。
 */
export function FromCanvasChip({ title, onRemove, className }: FromCanvasChipProps) {
  return (
    <span
      data-kind="from-canvas"
      title="这句话是画布里的按钮放进来的"
      className={cn(
        'inline-flex h-6 max-w-[260px] items-center gap-1 rounded-full border border-line-2 bg-pane pl-2 text-1 text-ink-2',
        onRemove ? 'pr-0.5' : 'pr-2',
        className,
      )}
    >
      <AppWindow className="size-3 shrink-0 text-ink-3" />
      <span className="shrink-0 text-ink-3">来自画布</span>
      <span className="min-w-0 truncate">{title}</span>
      {onRemove ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label="移除来自画布的标记"
          title="不带这个标记（当成你自己写的）"
          className="grid size-5 shrink-0 place-items-center rounded-full text-ink-3 transition-colors hover:bg-active hover:text-ink"
        >
          <X className="size-3" />
        </button>
      ) : null}
    </span>
  );
}

interface ResultCardChipProps {
  title: string;
  version: number;
  onRemove?: () => void;
  className?: string;
}

/**
 * T054.2：结果卡放进输入框的那张卡（240 宽、文件图标、两行：要求修改 · 第 N 版的结果 / 标题）。
 * 输入框里可删；发出去以后气泡上照原样画，不能删。
 */
export function ResultCardChip({ title, version, onRemove, className }: ResultCardChipProps) {
  return (
    <span
      data-kind="result-card-chip"
      title={'要求修改「' + title + '」第 ' + version + ' 版的结果'}
      className={cn(
        'inline-flex w-[240px] max-w-full items-center gap-2 rounded-btn border border-line-2 bg-pane px-2 py-1.5 text-2 text-ink-2',
        onRemove ? 'pr-1' : '',
        className,
      )}
    >
      <FileText className="size-4 shrink-0 text-ink-3" />
      <span className="flex min-w-0 flex-1 flex-col leading-4">
        <span className="truncate text-1 text-ink-3">
          要求修改 · 第 {version} 版的结果
        </span>
        <span className="truncate text-2 text-ink">{title}</span>
      </span>
      {onRemove ? (
        <button
          type="button"
          onClick={onRemove}
          aria-label="移除结果卡"
          className="grid size-5 shrink-0 place-items-center rounded-full text-ink-3 transition-colors hover:bg-active hover:text-ink"
        >
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
