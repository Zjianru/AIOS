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
import { createPortal } from 'react-dom';
import { FileText, Quote } from 'lucide-react';
import { toast } from 'sonner';
import { useKindStore } from '../kindStore';
import { fileMentions } from '../atGrammar';
import { attachmentGet, attachmentPut, fileReferences } from '@/data/service';
import type { FileReferenceEntry, SendAttachmentRef } from '@/data/service';
import { cn } from '@/lib/utils';
import { AttachmentPreview, FileCard, PasteCard, pasteStats } from './AttachmentCard';

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
  /** 发送；返回是否受理成功（成功才清草稿）。 */
  onSend: (text: string, options: ComposerSendOptions) => Promise<boolean>;
  onCanSendChange: (value: boolean) => void;
  /** 右侧控件（模型选择 + 发送/停止）。 */
  children?: React.ReactNode;
}

/** 本地记录文件的 MIME（发送时只带 id/name/bytes；预览用）。 */
interface ComposerAttachment extends SendAttachmentRef {
  mime: string;
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

/** 把文本按 @路径 / @"路径" 切段（高亮层用；和消息气泡共用 atGrammar 的语法）。 */
function highlightSegments(text: string): { value: string; mayPath: boolean }[] {
  const segments: { value: string; mayPath: boolean }[] = [];
  let cursor = 0;
  for (const mention of fileMentions(text)) {
    if (mention.start > cursor) segments.push({ value: text.slice(cursor, mention.start), mayPath: false });
    segments.push({ value: mention.raw, mayPath: true });
    cursor = mention.end;
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
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([]);
  const [pastePreview, setPastePreview] = useState<number | null>(null);
  const [filePreview, setFilePreview] = useState<{ id: string; text: string } | null>(null);
  const [atMenu, setAtMenu] = useState<AtMenuState | null>(null);
  const [quote, setQuote] = useState<string>('');
  /** T013 验收 F4：对话正文里的选区（页面级选区）与浮出的引用按钮位置。 */
  const [pageQuote, setPageQuote] = useState<{ text: string; top: number; left: number } | null>(null);
  const cardsRef = useRef<HTMLDivElement | null>(null);
  const highlightRef = useRef<HTMLDivElement | null>(null);
  // T013.8：草稿按会话存 localStorage（键 aios.kind.draft.<会话 id>）。
  const sessionId = useKindStore((state) => state.sessionId);
  const draftLoadedRef = useRef<string | null>(null);
  const touchedRef = useRef(false);
  const changedDuringSendRef = useRef(false);

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

  // T013 验收 F4：选中对话正文（回答、你的消息、工作过程）里的文字就浮出「引用」；
  // 输入框里的选区仍走 textarea 自己的 onSelect，两条路互不干扰。
  useEffect(() => {
    const inConversation = (node: Node | null): boolean => {
      if (!node) return false;
      const element = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
      if (!element || !element.closest('[data-pane="conversation"]')) return false;
      // 输入框在对话面板里，但它的选区由 textarea 自己处理。
      return element.closest('[data-kind="composer"]') === null;
    };
    const update = () => {
      const selection = window.getSelection();
      if (!selection || selection.isCollapsed || selection.rangeCount === 0) {
        setPageQuote(null);
        return;
      }
      const selected = selection.toString().trim();
      if (!selected || !inConversation(selection.anchorNode) || !inConversation(selection.focusNode)) {
        setPageQuote(null);
        return;
      }
      const range = selection.getRangeAt(0);
      const rects = range.getClientRects();
      const rect = rects.length > 0 ? rects[rects.length - 1] : range.getBoundingClientRect();
      if (rect.width === 0 && rect.height === 0) {
        setPageQuote(null);
        return;
      }
      // 按钮放在选区上方；上面放不下就放下面，左右夹在视口里。
      const width = 68;
      const top = rect.top >= 44 ? rect.top - 36 : Math.min(window.innerHeight - 44, rect.bottom + 8);
      const left = Math.min(Math.max(8, rect.right - width), window.innerWidth - width - 8);
      setPageQuote({ text: selected, top, left });
    };
    // 滚动/改窗口大小时选区位置变了，先收起按钮（不清用户选区）。
    const hide = () => setPageQuote(null);
    document.addEventListener('selectionchange', update);
    document.addEventListener('mouseup', update);
    document.addEventListener('scroll', hide, true);
    window.addEventListener('resize', hide);
    return () => {
      document.removeEventListener('selectionchange', update);
      document.removeEventListener('mouseup', update);
      document.removeEventListener('scroll', hide, true);
      window.removeEventListener('resize', hide);
    };
  }, []);

  /** T013 验收 F4：把正文选区按行加 > 前缀，追加到输入框（光标处，或末尾）。 */
  const applyPageQuote = useCallback(() => {
    if (!pageQuote) return;
    const element = inputRef.current;
    const current = element ? element.value : text;
    const caret = element && document.activeElement === element ? element.selectionStart : current.length;
    const quoted = pageQuote.text
      .split(String.fromCharCode(10))
      .map((line) => '> ' + line)
      .join(String.fromCharCode(10));
    const before = current.slice(0, caret);
    const after = current.slice(caret);
    // 引用块要独占行首：前后不是换行就各补一个。
    const prefix = before && !before.endsWith(String.fromCharCode(10)) ? String.fromCharCode(10) : '';
    const suffix = after && !after.startsWith(String.fromCharCode(10)) ? String.fromCharCode(10) : '';
    const inserted = prefix + quoted + suffix;
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    setText(before + inserted + after);
    setPageQuote(null);
    window.getSelection()?.removeAllRanges();
    requestAnimationFrame(() => {
      const input = inputRef.current;
      if (!input) return;
      input.focus();
      const position = (before + prefix + quoted).length;
      input.setSelectionRange(position, position);
      resize();
    });
  }, [pageQuote, resize, text]);

  // T013.8：会话 id 就绪后读一次草稿（读不出当空）。
  useEffect(() => {
    if (!sessionId || draftLoadedRef.current === sessionId) return;
    draftLoadedRef.current = sessionId;
    if (touchedRef.current) return; // ready 前已经输入：以用户输入为准，草稿不覆盖
    try {
      const raw = localStorage.getItem('aios.kind.draft.' + sessionId);
      if (!raw) return;
      const draft = JSON.parse(raw) as {
        text?: unknown;
        pastes?: unknown;
        attachments?: unknown;
      };
      if (typeof draft.text === 'string') setText(draft.text);
      if (Array.isArray(draft.pastes)) {
        setPastes(draft.pastes.filter((item): item is string => typeof item === 'string'));
      }
      if (Array.isArray(draft.attachments)) {
        const files = draft.attachments
          .filter((item): item is { id: string; name: string; bytes: number; mime?: string } => {
            return (
              Boolean(item) &&
              typeof (item as { id?: unknown }).id === 'string' &&
              typeof (item as { name?: unknown }).name === 'string' &&
              typeof (item as { bytes?: unknown }).bytes === 'number'
            );
          })
          .map((item) => ({ ...item, mime: item.mime ?? 'text/plain' }));
        setAttachments(files);
      }
    } catch {
      // 读不出或格式不对：当空草稿，不打扰用户。
    }
  }, [sessionId]);

  // 草稿变化就写（读完之后才写，避免把刚读到的覆盖成空）。
  useEffect(() => {
    if (!sessionId || draftLoadedRef.current !== sessionId) return;
    const draft = {
      text,
      pastes,
      attachments: attachments.map(({ id, name, bytes, mime }) => ({ id, name, bytes, mime })),
    };
    try {
      localStorage.setItem('aios.kind.draft.' + sessionId, JSON.stringify(draft));
    } catch {
      // 存不下（隐私模式）就算了。
    }
  }, [attachments, pastes, sessionId, text]);

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
      // 用 DOM 里的真实值（state 闭包可能比输入慢一拍）。
      const value = element.value;
      const caret = element.selectionStart;
      const inserted = insertReference(entry.path);
      touchedRef.current = true;
      changedDuringSendRef.current = true;
      const next = value.slice(0, atMenu.start) + inserted + value.slice(caret);
      setText(next);
      setAtMenu(null);
      requestAnimationFrame(() => {
        element.focus();
        const position = atMenu.start + inserted.length;
        element.setSelectionRange(position, position);
      });
    },
    [atMenu],
  );

  const intakeFiles = useCallback(async (files: File[]) => {
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    for (const file of files) {
      try {
        const data = await readAsBase64(file);
        const stored = await attachmentPut(file.name, data);
        setAttachments((previous) => [
          ...previous,
          { id: stored.id, name: stored.name, bytes: stored.bytes, mime: file.type || 'text/plain' },
        ]);
      } catch {
        toast('只能放文字文件', { description: file.name + ' 不是能读的文字文件' });
      }
    }
    requestAnimationFrame(() => {
      const cards = cardsRef.current;
      if (cards) cards.scrollLeft = cards.scrollWidth;
    });
  }, []);

  const submit = useCallback(async () => {
    if (offline) return;
    const value = text.trim();
    if (!value && pastes.length === 0 && attachments.length === 0) return;
    // T013.8：发送时先清空（马上能继续打），受理失败再把内容放回来（草稿不清）。
    const snapshot = { text, pastes, attachments, quote };
    changedDuringSendRef.current = false;
    setText('');
    setPastes([]);
    setAttachments([]);
    setAtMenu(null);
    setQuote('');
    setPageQuote(null);
    requestAnimationFrame(() => {
      resize();
      inputRef.current?.focus();
    });
    const accepted = await onSend(value, {
      pastes,
      attachments: attachments.map(({ id, name, bytes }) => ({ id, name, bytes })),
    });
    if (!accepted) {
      // 失败进 unsent：内容放回输入框（草稿也没动）；失败等待期间用户又输入了就不覆盖新的。
      if (changedDuringSendRef.current) return;
      setText(snapshot.text);
      setPastes(snapshot.pastes);
      setAttachments(snapshot.attachments);
      setQuote(snapshot.quote);
      return;
    }
    try {
      if (sessionId) localStorage.removeItem('aios.kind.draft.' + sessionId);
    } catch {
      // 忽略。
    }
  }, [attachments, offline, onSend, pastes, quote, resize, sessionId, text]);

  useImperativeHandle(ref, () => ({ submit: () => void submit(), focus: () => inputRef.current?.focus() }), [submit]);

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // @ 菜单打开：↑↓ 换高亮、Esc 关（Enter 在下面选高亮项）。
    if (atMenu && atMenu.items.length > 0) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        setAtMenu((previous) =>
          previous ? { ...previous, index: (previous.index + 1) % previous.items.length } : previous,
        );
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        setAtMenu((previous) =>
          previous
            ? { ...previous, index: (previous.index - 1 + previous.items.length) % previous.items.length }
            : previous,
        );
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setAtMenu(null);
        return;
      }
    }
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
    touchedRef.current = true;
    changedDuringSendRef.current = true;
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
              <PasteCard
                key={'paste-' + index}
                paste={paste}
                onOpen={() => setPastePreview(index)}
                onRemove={() => setPastes((previous) => previous.filter((_, item) => item !== index))}
              />
            ))}
            {attachments.map((file) => (
              <FileCard
                key={file.id + '-' + file.name}
                file={file}
                mime={file.mime}
                onOpen={() => {
                  void attachmentGet(file.id)
                    .then((result) => setFilePreview({ id: file.id, text: result.text ?? '（这个文件没有可显示的文字）' }))
                    .catch(() => toast('读不回这个文件', { description: file.name }));
                }}
                onRemove={() => setAttachments((previous) => previous.filter((item) => item.id !== file.id))}
              />
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
                    className="rounded-[3px] bg-active text-brand"
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
                touchedRef.current = true;
                changedDuringSendRef.current = true;
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
      {quote && !pageQuote ? (
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
            touchedRef.current = true;
            changedDuringSendRef.current = true;
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
      {pageQuote
        ? createPortal(
            <button
              type="button"
              data-kind="quote-button"
              // 按下时别清掉页面选区：click 里还要用 pageQuote.text。
              onMouseDown={(event) => event.preventDefault()}
              onClick={applyPageQuote}
              style={{ top: pageQuote.top, left: pageQuote.left }}
              className="fixed z-50 flex items-center gap-1 rounded-btn border border-line-2 bg-raised px-2 py-1 text-2 text-ink-2 shadow-[var(--shadow-pop)] hover:bg-hover hover:text-ink"
            >
              <Quote className="size-3" />
              引用
            </button>,
            document.body,
          )
        : null}
      {pastePreview !== null && pastes[pastePreview] !== undefined ? (
        <AttachmentPreview
          kind="paste"
          title={'粘贴内容 · ' + pasteStats(pastes[pastePreview]).lines + ' 行 · ' + pasteStats(pastes[pastePreview]).chars + ' 字'}
          text={pastes[pastePreview]}
          onClose={() => setPastePreview(null)}
          className="absolute bottom-full left-3 mb-1 w-[420px] max-w-[calc(100%-24px)]"
        />
      ) : null}
      {filePreview ? (
        <AttachmentPreview
          kind="file"
          title="文件全文"
          text={filePreview.text}
          onClose={() => setFilePreview(null)}
          className="absolute bottom-full left-3 mb-1 w-[420px] max-w-[calc(100%-24px)]"
        />
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
