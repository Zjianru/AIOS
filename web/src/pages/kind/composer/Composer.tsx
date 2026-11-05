/**
 * T013.7：输入框（照 DSH 的 composer 行为，页面用 textarea）。
 *
 * - 按键照 DSH keymap.ts:257-306：Shift+Enter 换行；Enter 发送；Cmd/Ctrl+Enter 发送；
 *   Alt+Enter 与 Ctrl+Meta / Shift+(Ctrl|Meta) 组合不消费；组字器里的 Enter 交给 IME；
 *   长按 Enter（repeat）不连发；
 * - 自动变高，最高 240px，超过后内部滚动；
 * - 粘贴超过 1000 个字符折成卡片（不进 textarea），发送时作为 pastes 的一项；
 * - 拖文件/粘贴文件出附件卡片（只收文字文件，二进制提示）；
 * - @ 菜单（打 @ 后按打的字列候选，选中插 @路径；带空格的路径加引号）；
 * - 选中文字浮出「引用」，点了把那几行变成 > 引用。
 */

import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { FileText, Quote, X } from 'lucide-react';
import { toast } from 'sonner';
import { attachmentPut, fileReferences } from '@/data/service';
import type { FileReferenceEntry, SendAttachmentRef } from '@/data/service';
import { cn } from '@/lib/utils';

const LARGE_PASTE_CHAR_THRESHOLD = 1000;
const MAX_HEIGHT = 240;

export interface ComposerSendOptions {
  pastes: string[];
  attachments: SendAttachmentRef[];
}

export interface ComposerHandle {
  /** 发送（按钮 / Enter 都走这里）。 */
  submit: () => void;
  focus: () => void;
}

interface ComposerProps {
  offline: boolean;
  onSend: (text: string, options: ComposerSendOptions) => void;
  onCanSendChange: (value: boolean) => void;
  /** 右侧控件（模型选择 + 发送/停止）。 */
  children?: React.ReactNode;
}

interface AtMenuState {
  start: number;
  query: string;
  items: FileReferenceEntry[];
  index: number;
}

function readAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result ?? '');
      resolve(result.slice(result.indexOf(',') + 1));
    };
    reader.onerror = () => reject(new Error('读文件失败'));
    reader.readAsDataURL(file);
  });
}

/** 把文本按 @路径 / @"路径" 切段（高亮层用）。 */
function highlightSegments(text: string): { value: string; mayPath: boolean }[] {
  const segments: { value: string; mayPath: boolean }[] = [];
  const pattern = /@(?:"[^"]*"|[^\s@]*)/g;
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    const start = match.index ?? 0;
    if (start > cursor) segments.push({ value: text.slice(cursor, start), mayPath: false });
    segments.push({ value: match[0], mayPath: true });
    cursor = start + match[0].length;
  }
  if (cursor < text.length) segments.push({ value: text.slice(cursor), mayPath: false });
  return segments;
}

/** 插入到光标的文字（带空格路径加引号）。 */
function insertReference(path: string): string {
  return path.includes(' ') ? '@"' + path + '"' : '@' + path;
}

export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer(
  { offline, onSend, onCanSendChange, children },
  ref,
) {
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const composingRef = useRef(false);
  const [text, setText] = useState('');
  const [pastes, setPastes] = useState<string[]>([]);
  const [attachments, setAttachments] = useState<SendAttachmentRef[]>([]);
  const [atMenu, setAtMenu] = useState<AtMenuState | null>(null);
  const [quote, setQuote] = useState<string>('');
  const cardsRef = useRef<HTMLDivElement | null>(null);
  const highlightRef = useRef<HTMLDivElement | null>(null);

  const canSend = text.trim().length > 0 || pastes.length > 0 || attachments.length > 0;

  useEffect(() => {
    onCanSendChange(canSend);
  }, [canSend, onCanSendChange]);

  const resize = useCallback(() => {
    const element = inputRef.current;
    if (!element) return;
    element.style.height = 'auto';
    const height = Math.min(element.scrollHeight, MAX_HEIGHT);
    element.style.height = height + 'px';
    element.style.overflowY = element.scrollHeight > MAX_HEIGHT ? 'auto' : 'hidden';
  }, []);

  useEffect(() => {
    resize();
  }, [text, resize]);

  const pruneAtMenu = useCallback((value: string, caret: number) => {
    const before = value.slice(0, caret);
    const match = /(?:^|\s)@([^\s@]*)$/.exec(before);
    if (!match) {
      setAtMenu(null);
      return;
    }
    const query = match[1];
    const start = caret - query.length - 1;
    setAtMenu((previous) => ({ start, query, items: previous?.query === query ? previous.items : [], index: 0 }));
  }, []);

  // 打 @ 之后查候选（防抖 120ms；服务端排序）。
  useEffect(() => {
    if (!atMenu) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      void fileReferences(atMenu.query)
        .then((items) => {
          if (!cancelled) setAtMenu((previous) => (previous ? { ...previous, items, index: 0 } : previous));
        })
        .catch(() => undefined);
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [atMenu?.query]);

  const pickReference = useCallback(
    (entry: FileReferenceEntry) => {
      const element = inputRef.current;
      if (!element || !atMenu) return;
      const caret = element.selectionStart;
      const inserted = insertReference(entry.path);
      setText(text.slice(0, atMenu.start) + inserted + text.slice(caret));
      setAtMenu(null);
      requestAnimationFrame(() => {
        element.focus();
        const position = atMenu.start + inserted.length;
        element.setSelectionRange(position, position);
      });
    },
    [atMenu, text],
  );

  const intakeFiles = useCallback(async (files: File[]) => {
    for (const file of files) {
      try {
        const data = await readAsBase64(file);
        const stored = await attachmentPut(file.name, data);
        setAttachments((previous) => [...previous, { id: stored.id, name: stored.name, bytes: stored.bytes }]);
      } catch {
        toast('只能放文字文件', { description: file.name + ' 不是能读的文字文件' });
      }
    }
    requestAnimationFrame(() => {
      const cards = cardsRef.current;
      if (cards) cards.scrollLeft = cards.scrollWidth;
    });
  }, []);

  const submit = useCallback(() => {
    if (offline) return;
    const value = text.trim();
    if (!value && pastes.length === 0 && attachments.length === 0) return;
    onSend(value, { pastes, attachments });
    setText('');
    setPastes([]);
    setAttachments([]);
    setAtMenu(null);
    setQuote('');
    requestAnimationFrame(() => {
      resize();
      inputRef.current?.focus();
    });
  }, [attachments, offline, onSend, pastes, resize, text]);

  useImperativeHandle(ref, () => ({ submit, focus: () => inputRef.current?.focus() }), [submit]);

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter') return;
    // 组字器里的 Enter 是选字，不发送也不换行（浏览器自己处理）。
    if (event.nativeEvent.isComposing || composingRef.current) return;
    // Alt+Enter、Ctrl+Meta、Shift+(Ctrl|Meta) 不消费（交给别处）。
    if (event.altKey || (event.ctrlKey && event.metaKey) || (event.shiftKey && (event.ctrlKey || event.metaKey))) return;
    // Shift+Enter 保持原生换行。
    if (event.shiftKey) return;
    // @ 菜单打开且有候选：Enter 选高亮项。
    if (atMenu && atMenu.items.length > 0) {
      event.preventDefault();
      const entry = atMenu.items[Math.min(atMenu.index, atMenu.items.length - 1)];
      if (entry) pickReference(entry);
      return;
    }
    event.preventDefault();
    // 长按 Enter 不连发。
    if (event.repeat) return;
    submit();
  };

  const onInput = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    const element = event.currentTarget;
    setText(element.value);
    pruneAtMenu(element.value, element.selectionStart);
    resize();
  };

  return (
    <div
      data-kind="composer"
      className="relative"
      onDragOver={(event) => {
        event.preventDefault();
      }}
      onDrop={(event) => {
        event.preventDefault();
        // 目录不收（只收文字文件）：webkitGetAsEntry 能识别拖进来的目录。
        const items = Array.from(event.dataTransfer.items ?? []);
        const hasDirectory = items.some((item) => {
          const entry = (
            item as DataTransferItem & { webkitGetAsEntry?: () => { isDirectory?: boolean } | null }
          ).webkitGetAsEntry?.();
          return entry?.isDirectory === true;
        });
        if (hasDirectory) {
          toast('只能放文字文件', { description: '文件夹不行，一次拖一个文字文件' });
          return;
        }
        const files = Array.from(event.dataTransfer.files ?? []);
        if (files.length > 0) void intakeFiles(files);
      }}
    >
      <div className="rounded-card border border-line-2 bg-raised/70 px-3 py-2 transition-colors">
        {pastes.length > 0 || attachments.length > 0 ? (
          <div ref={cardsRef} className="mb-1.5 flex gap-1.5 overflow-x-auto pb-1" data-kind="composer-cards">
            {pastes.map((paste, index) => (
              <span
                key={'paste-' + index}
                data-kind="paste-card"
                className="flex shrink-0 items-center gap-1 rounded-btn border border-line-2 bg-pane px-2 py-1 text-2 text-ink-2"
              >
                <FileText className="size-3" />
                <span className="max-w-[220px] truncate">{paste.replace(/\s+/g, ' ').slice(0, 80)}</span>
                <button
                  type="button"
                  aria-label="移除粘贴"
                  onClick={() => setPastes((previous) => previous.filter((_, item) => item !== index))}
                  className="text-ink-3 hover:text-ink"
                >
                  <X className="size-3" />
                </button>
              </span>
            ))}
            {attachments.map((file) => (
              <span
                key={file.id}
                data-kind="file-card"
                className="flex shrink-0 items-center gap-1 rounded-btn border border-line-2 bg-pane px-2 py-1 text-2 text-ink-2"
              >
                <FileText className="size-3" />
                <span className="max-w-[180px] truncate">{file.name}</span>
                <span className="text-ink-3">{Math.max(1, Math.round(file.bytes / 1024))}KB</span>
                <button
                  type="button"
                  aria-label="移除文件"
                  onClick={() => setAttachments((previous) => previous.filter((item) => item.id !== file.id))}
                  className="text-ink-3 hover:text-ink"
                >
                  <X className="size-3" />
                </button>
              </span>
            ))}
          </div>
        ) : null}
        <div className="flex items-end gap-2">
          <div className="relative min-w-0 flex-1">
            {/* T013.7：下层 @路径 高亮（textarea 文字透明，看到的彩色文字来自这层）。 */}
            <div
              ref={highlightRef}
              data-kind="at-highlight"
              aria-hidden
              className="pointer-events-none absolute inset-0 overflow-hidden whitespace-pre-wrap break-words text-4 leading-6"
            >
              {highlightSegments(text).map((segment, index) =>
                segment.mayPath ? (
                  <span
                    key={index}
                    data-at-token={segment.value}
                    className="rounded-[3px] bg-active px-0.5 text-brand"
                  >
                    {segment.value}
                  </span>
                ) : (
                  <span key={index}>{segment.value}</span>
                ),
              )}
              {text.endsWith(String.fromCharCode(10)) ? ' ' : null}
            </div>
            <textarea
            ref={inputRef}
            rows={1}
            data-kind="composer-input"
            placeholder={offline ? '连上之后再发…' : '和 Kind 说…'}
            value={text}
            onChange={onInput}
            onKeyDown={onKeyDown}
            onScroll={(event) => {
              if (highlightRef.current) highlightRef.current.scrollTop = event.currentTarget.scrollTop;
            }}
            onCompositionStart={() => {
              composingRef.current = true;
            }}
            onCompositionEnd={() => {
              composingRef.current = false;
            }}
            onPaste={(event) => {
              const clipboard = event.clipboardData;
              if (!clipboard) return;
              const files = Array.from(clipboard.files ?? []);
              if (files.length > 0) {
                event.preventDefault();
                void intakeFiles(files);
                return;
              }
              const pasted = clipboard.getData('text/plain');
              if (pasted.length > LARGE_PASTE_CHAR_THRESHOLD) {
                event.preventDefault();
                setPastes((previous) => [...previous, pasted]);
                requestAnimationFrame(() => {
                  const cards = cardsRef.current;
                  if (cards) cards.scrollLeft = cards.scrollWidth;
                });
              }
            }}
            onSelect={(event) => {
              const element = event.currentTarget;
              const selected = element.value.slice(element.selectionStart, element.selectionEnd).trim();
              setQuote(selected);
            }}
            className="relative min-h-6 w-full resize-none bg-transparent text-4 leading-6 text-transparent caret-ink outline-none placeholder:text-ink-3"
            style={{ maxHeight: MAX_HEIGHT }}
            />
          </div>
          {children}
        </div>
      </div>
      {quote ? (
        <button
          type="button"
          data-kind="quote-button"
          onClick={() => {
            const element = inputRef.current;
            if (!element) return;
            const start = element.selectionStart;
            const end = element.selectionEnd;
            const quoted = element.value
              .slice(start, end)
              .split(String.fromCharCode(10))
              .map((line) => '> ' + line)
              .join(String.fromCharCode(10));
            setText(element.value.slice(0, start) + quoted + element.value.slice(end));
            setQuote('');
            element.focus();
          }}
          className="absolute -top-8 right-0 flex items-center gap-1 rounded-btn border border-line-2 bg-raised px-2 py-1 text-2 text-ink-2 shadow-[var(--shadow-pop)] hover:bg-hover hover:text-ink"
        >
          <Quote className="size-3" />
          引用
        </button>
      ) : null}
      {atMenu && atMenu.items.length > 0 ? (
        <div
          data-kind="at-menu"
          className="absolute bottom-full left-3 mb-1 max-h-[240px] w-80 overflow-y-auto rounded-card border border-line-2 bg-raised p-1 shadow-[var(--shadow-pop)]"
        >
          {atMenu.items.map((item, index) => (
            <button
              key={item.path}
              type="button"
              onMouseDown={(event) => {
                event.preventDefault();
                pickReference(item);
              }}
              className={cn(
                'flex w-full items-center gap-2 rounded-btn px-2 py-1 text-left text-2',
                index === atMenu.index ? 'bg-active text-ink' : 'text-ink-2 hover:bg-hover hover:text-ink',
              )}
            >
              <FileText className="size-3 shrink-0" />
              <span className="min-w-0 flex-1 truncate font-mono">{item.path}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
});
