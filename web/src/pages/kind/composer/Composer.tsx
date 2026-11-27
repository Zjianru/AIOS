/**
 * T013.7：输入框（照 DSH 的 composer 行为，页面用 textarea）。
 *
 * - 按键照 DSH keymap.ts:257-306：Shift+Enter 换行；Enter 发送；Cmd/Ctrl+Enter 发送；
 *   Alt+Enter 与 Ctrl+Meta / Shift+(Ctrl|Meta) 组合不消费；组字器里的 Enter 交给 IME；
 *   长按 Enter（repeat）不连发；
 * - 自动变高，最高 240px，超过后内部滚动；
 * - 粘贴超过 1000 个字符折成卡片（不进 textarea），发送时作为 pastes 的一项；
 * - 拖文件/粘贴文件出附件卡片（图片/文字/其他非文字文件都收，文件夹不收）；
 * - @ 菜单（打 @ 后按打的字列候选，选中插 @路径；带空格的路径加引号）；
 * - 选中文字浮出「引用」，点了变成输入框上方的引用卡；发送时才按行变成 > 行。
 */

import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { FileText, MapPin, Paperclip, Quote, X } from 'lucide-react';
import { toast } from 'sonner';
import { useKindStore } from '../kindStore';
import { fileMentions } from '../atGrammar';
import type { QueueRestore } from '../inbox';
import { mimeFromFileName } from '../fileMime.ts';
import { canRecallCleared } from '../rewind.ts';
import { takePendingDraft, takePendingQuote } from '@/lib/pendingDraft';
import type { RewindSource } from '../rewind.ts';
import { attachmentGet, attachmentPut, fileReferences, peekAvailableModels } from '@/data/service';
import type { FileReferenceEntry, FileReferenceObject, SendAttachmentRef } from '@/data/service';
import { cn } from '@/lib/utils';
import {
  AttachmentPreview,
  FileCard,
  ImageCard,
  imageMimeSupported,
  FromCanvasChip,
  PasteCard,
  QuoteCard,
  ResultCardChip,
  pasteStats,
  ObjectRefCard,
} from './AttachmentCard';

const LARGE_PASTE_CHAR_THRESHOLD = 1000;
const MAX_HEIGHT = 240;

export interface ComposerSendOptions {
  pastes: string[];
  attachments: SendAttachmentRef[];
  /** @ 选中的任务/报告/项目引用片。 */
  objectRefs?: FileReferenceObject[];
  /** 选中文字引用（带来源的走服务端 Quote 块；不带来源的仍拼进正文）。 */
  quotes?: ComposerQuote[];
  /** T015.4：画布放进来的那一句带着画布身份；没有就是自己写的。 */
  canvas?: { id: string; title: string };
  /** T054.2：结果卡「要求修改」带进来的那张卡。 */
  resultCard?: { taskId: string; title: string; version: number };
  /** （N2）：一次放回多条时按顺序的列表（单个时仍给 canvas/resultCard 兼容）。 */
  canvases?: { id: string; title: string }[];
  resultCards?: { taskId: string; title: string; version: number }[];
}

export interface ComposerHandle {
  /** 发送（按钮 / Enter 都走这里）。 */
  submit: () => void;
  focus: () => void;
  /** T015.4：画布里的按钮把一句话放进输入框（接在已经写的后面，不发送）。 */
  putFromCanvas: (text: string, canvas: { id: string; title: string }) => void;
  /** 输入框里有没有东西（两下 Esc 决定清空还是开菜单）。 */
  hasDraftContent: () => boolean;
  /** 两下 Esc 清空：草稿压进 cleared，↑ 能取回。 */
  clearToHistory: () => void;
  /** ↑ 取回最近清掉的一条。 */
  recallCleared: () => boolean;
  /** 恢复对话时把原文（字/粘贴/附件/结果卡/画布标记）放回输入框。 */
  putBack: (source: RewindSource) => void;
  /** 任务对话/报告里选中的一段话放进输入框（带来源）。 */
  putQuote: (quote: ComposerQuote) => void;
}

interface ClearedDraft {
  text: string;
  quotes: ComposerQuote[];
  pastes: string[];
  attachments: { id: string; name: string; bytes: number; mime: string }[];
  fromCanvas: { id: string; title: string } | null;
  resultCard: { taskId: string; title: string; version: number } | null;
  extraCanvases: { id: string; title: string }[];
  extraResultCards: { taskId: string; title: string; version: number }[];
}

interface ComposerProps {
  offline: boolean;
  /** 发送；返回是否受理成功（成功才清草稿）。 */
  onSend: (text: string, options: ComposerSendOptions) => Promise<boolean>;
  onCanSendChange: (value: boolean) => void;
  /** 右侧控件（模型选择 + 发送/停止）。 */
  children?: React.ReactNode;
  /**
   * T028.5：草稿的作用域（默认 Kind 会话 id）。任务对话传 task-<taskId>，
   * 草稿按任务分开存，切走再回来还在。
   */
  draftKey?: string;
  /** T028.5：占位文字（任务对话在跑/没在跑时不同）。 */
  placeholder?: string;
  /** 从日志恢复的排队消息（被停止清掉的那些）；每条 seq 最多放回一次。 */
  queueRestores?: QueueRestore[];
  /** 输入框里的页面位置标签（Kind 传「现在」；任务对话不传）。 */
  pageTag?: string;
  /** 当前模型能不能看图；看不了时图片卡照实写，仍允许发送。 */
  imagesOk?: boolean;
}

/** 输入框里的一段引用（source 为空=Kind 里普通选段，仍拼进正文）。 */
export interface ComposerQuote {
  text: string;
  source?: {
    kind: string;
    id: string;
    title: string;
    version?: number | null;
    taskId?: string | null;
  } | null;
}

/** 本地记录文件的 MIME（发送时只带 id/name/bytes；预览用）。 */
interface ComposerAttachment extends SendAttachmentRef {
  mime: string;
}

type AtMenuItem =
  | { group: 'file'; entry: FileReferenceEntry }
  | { group: 'task' | 'report' | 'project'; object: FileReferenceObject };

interface AtMenuState {
  start: number;
  query: string;
  items: AtMenuItem[];
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

/** G3：一条引用按行加 `> `，多条之间空一行；发送时才拼进消息。 */
function quotedLines(quote: string): string {
  return quote
    .split(String.fromCharCode(10))
    .map((line) => '> ' + line)
    .join(String.fromCharCode(10));
}

export const Composer = forwardRef<ComposerHandle, ComposerProps>(function Composer(
  { offline, onSend, onCanSendChange, children, draftKey, placeholder, queueRestores = [], pageTag, imagesOk },
  ref,
) {
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const composingRef = useRef(false);
  const [text, setText] = useState('');
  const [quotes, setQuotes] = useState<ComposerQuote[]>([]);
  const [pastes, setPastes] = useState<string[]>([]);
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([]);
  const [objectRefs, setObjectRefs] = useState<FileReferenceObject[]>([]);
  const kindModel = useKindStore((state) => state.model);
  // 查不到/读不到都按看不了图（不猜）；加载完成前也不显示缩略图。
  const [modelSeesImages, setModelSeesImages] = useState(false);
  useEffect(() => {
    let alive = true;
    void peekAvailableModels(kindModel.connectionId)
      .then((result) => {
        const info = result.models.find((model) => model.id === kindModel.modelId);
        if (alive) setModelSeesImages(Boolean(info?.input?.includes('image')));
      })
      .catch(() => {
        if (alive) setModelSeesImages(false);
      });
    return () => {
      alive = false;
    };
  }, [kindModel.connectionId, kindModel.modelId]);
  /** T015.4：这句话是画布放进来的（发出去时带标记，气泡上留着）。 */
  const [fromCanvas, setFromCanvas] = useState<{ id: string; title: string } | null>(null);
  // （N2）：一次放回多条画布标记/结果卡；第一条走上面的单值状态（兼容旧 UI），其余放这里。
  const [extraCanvases, setExtraCanvases] = useState<{ id: string; title: string }[]>([]);
  const [extraResultCards, setExtraResultCards] = useState<
    { taskId: string; title: string; version: number }[]
  >([]);
  // 两下 Esc 清掉的草稿，↑ 取最近一条；跟草稿一起存 localStorage。
  const [cleared, setCleared] = useState<ClearedDraft[]>([]);
  /** T054.2：结果卡「要求修改」放进输入框的那张卡（一次一张，再点别的卡就换掉）。 */
  const [resultCard, setResultCard] = useState<{
    taskId: string;
    title: string;
    version: number;
  } | null>(null);
  const pendingResultCard = useKindStore((state) => state.pendingResultCard);
  const clearPendingResultCard = useKindStore((state) => state.clearPendingResultCard);
  const setComposerResultCard = useKindStore((state) => state.setComposerResultCard);
  const [pastePreview, setPastePreview] = useState<number | null>(null);
  const [filePreview, setFilePreview] = useState<{ id: string; text: string } | null>(null);
  const [atMenu, setAtMenu] = useState<AtMenuState | null>(null);
  const [quote, setQuote] = useState<string>('');
  /** T013 验收 F4：对话正文里的选区（页面级选区）与浮出的引用按钮位置。 */
  const [pageQuote, setPageQuote] = useState<{ text: string; top: number; left: number } | null>(null);
  const cardsRef = useRef<HTMLDivElement | null>(null);
  const highlightRef = useRef<HTMLDivElement | null>(null);
  // T013.8：草稿按会话存 localStorage（键 aios.kind.draft.<会话 id>）。
  // T028.5：任务对话用 draftKey=task-<taskId>，草稿按任务分开存。
  const sessionId = useKindStore((state) => state.sessionId);
  const draftScope = draftKey ?? sessionId;
  const draftLoadedRef = useRef<string | null>(null);
  const touchedRef = useRef(false);
  const changedDuringSendRef = useRef(false);
  /** 已经放回过的 canceled splice seq；跟草稿一起存，刷新不再重复放回。 */
  // 每条放回用 seq+index 唯一标识（N2 逐条放回；同一 splice 可能有多条消息）。
  const restoredKeysRef = useRef<string[]>([]);

  const canSend =
    text.trim().length > 0 ||
    quotes.length > 0 ||
    pastes.length > 0 ||
    attachments.length > 0 ||
    objectRefs.length > 0;
  const [pageTagDismissed, setPageTagDismissed] = useState(false);

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

  // T054.2：结果卡「要求修改」把卡交给输入框：消费后聚焦，卡可删，一次只留一张。
  useEffect(() => {
    if (!pendingResultCard) return;
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    setResultCard(pendingResultCard);
    clearPendingResultCard();
    requestAnimationFrame(() => {
      resize();
      inputRef.current?.focus();
    });
  }, [clearPendingResultCard, pendingResultCard, resize]);

  // T055.3 G1：输入框里的结果卡同步到 kindStore，结果卡据此显示「已带进输入框」；
  // 发送/移除/清空时 resultCard 置空，这里跟着清。草稿恢复在同一个 state 上，刷新后自动重建。
  useEffect(() => {
    setComposerResultCard(resultCard);
  }, [resultCard, setComposerResultCard]);
  useEffect(
    () => () => setComposerResultCard(null),
    [setComposerResultCard],
  );

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

  /** T013 验收 F4 + G3：正文选区变成输入框上方的引用卡，不写进输入框。 */
  const applyPageQuote = useCallback(() => {
    if (!pageQuote) return;
    const selected = pageQuote.text.trim();
    if (!selected) {
      setPageQuote(null);
      return;
    }
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    setQuotes((previous) => [...previous, { text: selected }]);
    setPageQuote(null);
    window.getSelection()?.removeAllRanges();
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [pageQuote]);

  // 记忆页「加一条」没有页面直写协议，经这个键把一句草稿带进 Kind 输入框。
  // T013.8：会话 id 就绪后读一次草稿（读不出当空）。
  useEffect(() => {
    if (!draftScope || draftLoadedRef.current === draftScope) return;
    // T028.5：草稿作用域换了（任务 A → 任务 B）先清空，别把上一个任务的草稿带过去；
    // 第一次拿到会话 id（Kind）不算换作用域，用户已经打的字要留着。
    if (draftLoadedRef.current !== null) {
      setText('');
      setQuotes([]);
      setPastes([]);
      setAttachments([]);
      setFromCanvas(null);
      setResultCard(null);
      setQuote('');
      touchedRef.current = false;
      changedDuringSendRef.current = false;
    }
    draftLoadedRef.current = draftScope;
    if (touchedRef.current) return; // ready 前已经输入：以用户输入为准，草稿不覆盖
    try {
      const raw = localStorage.getItem('aios.kind.draft.' + draftScope);
      const pendingText = takePendingDraft();
      const pendingQuote = takePendingQuote();
      if (!raw) {
        if (pendingText !== null) setText(pendingText);
        if (pendingQuote) setQuotes([{ text: pendingQuote.text, source: pendingQuote.source }]);
        return;
      }
      const draft = JSON.parse(raw) as {
        text?: unknown;
        quotes?: unknown;
        pastes?: unknown;
        attachments?: unknown;
        canvas?: unknown;
        resultCard?: unknown;
        canvases?: unknown;
        resultCards?: unknown;
        restoredKeys?: unknown;
        cleared?: unknown;
      };
      if (typeof draft.text === 'string') setText(pendingText ?? draft.text);
      const fromCanvas = draft.canvas as { id?: unknown; title?: unknown } | null | undefined;
      if (fromCanvas && typeof fromCanvas.id === 'string' && typeof fromCanvas.title === 'string') {
        setFromCanvas({ id: fromCanvas.id, title: fromCanvas.title });
      }
      if (Array.isArray(draft.canvases)) {
        const canvases = (draft.canvases as unknown[]).filter(
          (item): item is { id: string; title: string } =>
            Boolean(item) &&
            typeof (item as { id?: unknown }).id === 'string' &&
            typeof (item as { title?: unknown }).title === 'string',
        );
        if (canvases.length > 0) {
          setFromCanvas(canvases[0] ?? null);
          setExtraCanvases(canvases.slice(1));
        }
      }
      if (Array.isArray(draft.resultCards)) {
        const cards = (draft.resultCards as unknown[]).filter(
          (item): item is { taskId: string; title: string; version: number } =>
            Boolean(item) &&
            typeof (item as { taskId?: unknown }).taskId === 'string' &&
            typeof (item as { title?: unknown }).title === 'string' &&
            typeof (item as { version?: unknown }).version === 'number',
        );
        if (cards.length > 0) {
          setResultCard(cards[0] ?? null);
          setExtraResultCards(cards.slice(1));
        }
      }
      if (Array.isArray(draft.quotes)) {
        const restored: ComposerQuote[] = [];
        for (const item of draft.quotes) {
          if (typeof item === 'string') restored.push({ text: item });
          else if (item && typeof item === 'object' && typeof (item as { text?: unknown }).text === 'string') {
            restored.push(item as ComposerQuote);
          }
        }
        const pending = takePendingQuote();
        if (pending) restored.push({ text: pending.text, source: pending.source });
        setQuotes(restored);
      } else {
        const pending = takePendingQuote();
        if (pending) setQuotes([{ text: pending.text, source: pending.source }]);
      }
      if (Array.isArray(draft.pastes)) {
        setPastes(draft.pastes.filter((item): item is string => typeof item === 'string'));
      }
      const card = draft.resultCard as
        | { taskId?: unknown; title?: unknown; version?: unknown }
        | null
        | undefined;
      if (
        card &&
        typeof card.taskId === 'string' &&
        typeof card.title === 'string' &&
        typeof card.version === 'number'
      ) {
        setResultCard({ taskId: card.taskId, title: card.title, version: card.version });
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
          .map((item) => ({ ...item, mime: item.mime ?? mimeFromFileName(item.name) ?? 'text/plain' }));
        setAttachments(files);
      }
      if (Array.isArray(draft.restoredKeys)) {
        restoredKeysRef.current = draft.restoredKeys.filter(
          (item): item is string => typeof item === 'string',
        );
      }
      if (Array.isArray(draft.cleared)) {
        setCleared(
          draft.cleared.filter(
            (item): item is ClearedDraft =>
              Boolean(item) &&
              typeof item === 'object' &&
              typeof (item as { text?: unknown }).text === 'string' &&
              Array.isArray((item as { pastes?: unknown }).pastes),
          ),
        );
      }
    } catch {
      // 读不出或格式不对：当空草稿，不打扰用户。
    }
  }, [draftScope]);

  // 草稿变化就写（读完之后才写，避免把刚读到的覆盖成空）。
  useEffect(() => {
    if (!draftScope || draftLoadedRef.current !== draftScope) return;
    const draft = {
      text,
      quotes,
      pastes,
      attachments: attachments.map(({ id, name, bytes, mime }) => ({ id, name, bytes, mime })),
      canvas: fromCanvas,
      resultCard,
      canvases: [...(fromCanvas ? [fromCanvas] : []), ...extraCanvases],
      resultCards: [...(resultCard ? [resultCard] : []), ...extraResultCards],
      restoredKeys: restoredKeysRef.current,
      cleared,
    };
    try {
      localStorage.setItem('aios.kind.draft.' + draftScope, JSON.stringify(draft));
    } catch {
      // 存不下（隐私模式）就算了。
    }
  }, [attachments, cleared, draftScope, extraCanvases, extraResultCards, fromCanvas, pastes, quotes, resultCard, text]);

  // 停止时内核照 DSH clear 清掉排队消息；把 canceled splice 里被删的、
  // 页面从日志折叠回来的用户消息放回输入框。每条 seq 最多放回一次：放回过的 seq 跟草稿
  // 一起存，刷新不会把用户删掉的字又塞回来；打开页面时只补还没放回过的（页面没开着时按的停止）。
  useEffect(() => {
    if (!draftScope || draftLoadedRef.current !== draftScope) return;
    const pending = queueRestores
      .filter((restore) => !restoredKeysRef.current.includes(restore.key))
      .sort((left, right) => left.seq - right.seq);
    if (pending.length === 0) return;
    restoredKeysRef.current = restoredKeysRef.current.concat(
      pending.map((restore) => restore.key),
    );
    const restoredText = pending
      .map((restore) => restore.text)
      .filter((item) => item.length > 0)
      .join(String.fromCharCode(10));
    const pastes = pending.flatMap((restore) => restore.pastes);
    const attachments = pending.flatMap((restore) => restore.attachments);
    const resultCards = pending.flatMap((restore) => restore.resultCards);
    const canvases = pending.flatMap((restore) => restore.canvases);
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    if (restoredText) {
      setText((previous) =>
        previous.trim()
          ? previous.replace(/\s+$/, '') + String.fromCharCode(10) + restoredText
          : restoredText,
      );
    }
    if (pastes.length > 0) setPastes((previous) => previous.concat(pastes));
    if (attachments.length > 0) {
      setAttachments((previous) =>
        previous.concat(
          attachments.map((item) => ({
            ...item,
            mime: item.mime ?? mimeFromFileName(item.name) ?? 'text/plain',
          })),
        ),
      );
    }
    if (resultCards.length > 0) {
      setResultCard(resultCards[0] ?? null);
      setExtraResultCards((previous) => previous.concat(resultCards.slice(1)));
    }
    if (canvases.length > 0) {
      setFromCanvas(canvases[0] ?? null);
      setExtraCanvases((previous) => previous.concat(canvases.slice(1)));
    }
    requestAnimationFrame(() => {
      resize();
      inputRef.current?.focus();
    });
  }, [draftScope, queueRestores, resize]);

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
        .then((result) => {
          const items: AtMenuItem[] = [
            ...result.references.map((entry): AtMenuItem => ({ group: 'file', entry })),
            ...result.objects.map((object): AtMenuItem => ({ group: object.kind as 'task' | 'report' | 'project', object })),
          ];
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
    (item: AtMenuItem) => {
      const element = inputRef.current;
      if (!element || !atMenu) return;
      // 用 DOM 里的真实值（state 闭包可能比输入慢一拍）。
      const value = element.value;
      const caret = element.selectionStart;
      touchedRef.current = true;
      changedDuringSendRef.current = true;
      if (item.group === 'file') {
        const inserted = insertReference(item.entry.path);
        const next = value.slice(0, atMenu.start) + inserted + value.slice(caret);
        setText(next);
        setAtMenu(null);
        requestAnimationFrame(() => {
          element.focus();
          const position = atMenu.start + inserted.length;
          element.setSelectionRange(position, position);
        });
        return;
      }
      // 对象引用片不进正文：把打出来的 @query 去掉，卡片挂到附件那一排。
      const next = value.slice(0, atMenu.start) + value.slice(caret);
      setText(next);
      setObjectRefs((previous) =>
        previous.some((object) => object.kind === item.object.kind && object.id === item.object.id)
          ? previous
          : previous.concat(item.object),
      );
      setAtMenu(null);
      requestAnimationFrame(() => {
        element.focus();
        const position = atMenu.start;
        element.setSelectionRange(position, position);
      });
    },
    [atMenu],
  );

  useEffect(() => {
    const onQuote = (event: Event) => {
      const detail = (event as CustomEvent<ComposerQuote>).detail;
      if (!detail || typeof detail.text !== 'string') return;
      touchedRef.current = true;
      changedDuringSendRef.current = true;
      setQuotes((previous) => previous.concat(detail));
    };
    window.addEventListener('aios:quote', onQuote);
    return () => window.removeEventListener('aios:quote', onQuote);
  }, []);

  const intakeFiles = useCallback(async (files: File[]) => {
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    for (const file of files) {
      try {
        const data = await readAsBase64(file);
        const stored = await attachmentPut(file.name, data);
        setAttachments((previous) => [
          ...previous,
          {
            id: stored.id,
            name: stored.name,
            bytes: stored.bytes,
            mime: file.type || mimeFromFileName(file.name) || 'application/octet-stream',
          },
        ]);
      } catch {
        toast('附件没存上', { description: file.name + ' 没有被存进只读副本' });
      }
    }
    requestAnimationFrame(() => {
      const cards = cardsRef.current;
      if (cards) cards.scrollLeft = cards.scrollWidth;
    });
  }, []);

  const submit = useCallback(async () => {
    if (offline) {
      // 断线时不静默丢字——草稿留在输入框，连上后再发。
      toast('没连上内核，字还在，连上后再发');
      return;
    }
    // G3：引用卡在发送时才按行变成 `> `，排在打的字前面（气泡里画成引用块）。
    // G3：普通引用卡在发送时按行加引用前缀拼进正文；带来源的走 quotes 字段（服务端 Quote 块）。
    const plainQuotes = quotes.filter((item) => !item.source).map((item) => item.text);
    const sourcedQuotes = quotes
      .filter((item) => item.source)
      .map((item) => ({ text: item.text, source: item.source! }));
    const quoted = plainQuotes.map(quotedLines).join(String.fromCharCode(10) + String.fromCharCode(10));
    const value = [quoted, text.trim()]
      .filter((part) => part.length > 0)
      .join(String.fromCharCode(10));
    if (
      !value &&
      pastes.length === 0 &&
      attachments.length === 0 &&
      objectRefs.length === 0 &&
      sourcedQuotes.length === 0
    )
      return;
    // T013.8：发送时先清空（马上能继续打），受理失败再把内容放回来（草稿不清）。
    const snapshot = { text, quotes, pastes, attachments, objectRefs, quote, fromCanvas, resultCard, extraCanvases, extraResultCards };
    changedDuringSendRef.current = false;
    setText('');
    setQuotes([]);
    setPastes([]);
    setAttachments([]);
    setObjectRefs([]);
    setAtMenu(null);
    setQuote('');
    setPageQuote(null);
    setFromCanvas(null);
    setResultCard(null);
    setExtraCanvases([]);
    setExtraResultCards([]);
    requestAnimationFrame(() => {
      resize();
      inputRef.current?.focus();
    });
    const accepted = await onSend(value, {
      pastes,
      attachments: attachments.map(({ id, name, bytes, mime }) => ({ id, name, bytes, mime: mime ?? null })),
      objectRefs,
      quotes: sourcedQuotes,
      canvases: [...(fromCanvas ? [fromCanvas] : []), ...extraCanvases],
      resultCards: [...(resultCard ? [resultCard] : []), ...extraResultCards],
    });
    if (!accepted) {
      // 失败进 unsent：内容放回输入框（草稿也没动）；失败等待期间用户又输入了就不覆盖新的。
      if (changedDuringSendRef.current) return;
      setText(snapshot.text);
      setQuotes(snapshot.quotes);
      setPastes(snapshot.pastes);
      setAttachments(snapshot.attachments);
      setObjectRefs(snapshot.objectRefs);
      setQuote(snapshot.quote);
      setFromCanvas(snapshot.fromCanvas);
      setResultCard(snapshot.resultCard);
      setExtraCanvases(snapshot.extraCanvases);
      setExtraResultCards(snapshot.extraResultCards);
      return;
    }
    try {
      if (draftScope) localStorage.removeItem('aios.kind.draft.' + draftScope);
    } catch {
      // 忽略。
    }
  }, [
    attachments,
    draftScope,
    extraCanvases,
    extraResultCards,
    fromCanvas,
    objectRefs,
    offline,
    onSend,
    pastes,
    quote,
    quotes,
    resize,
    resultCard,
    text,
  ]);


  const hasDraftContent = useCallback(
    () =>
      Boolean(
        text.trim() ||
          quotes.length > 0 ||
          pastes.length > 0 ||
          attachments.length > 0 ||
          objectRefs.length > 0 ||
          fromCanvas ||
          resultCard ||
          extraCanvases.length > 0 ||
          extraResultCards.length > 0,
      ),
    [
      attachments.length,
      extraCanvases.length,
      extraResultCards.length,
      fromCanvas,
      objectRefs.length,
      pastes.length,
      quotes.length,
      resultCard,
      text,
    ],
  );

  /** 两下 Esc 清空输入框；清掉的东西压进 cleared，↑ 能取回。 */
  const clearToHistory = useCallback(() => {
    if (!hasDraftContent()) return;
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    setCleared((previous) =>
      previous.concat({
        text,
        quotes,
        pastes,
        attachments: attachments.map(({ id, name, bytes, mime }) => ({ id, name, bytes, mime })),
        fromCanvas,
        resultCard,
        extraCanvases,
        extraResultCards,
      }),
    );
    setText('');
    setQuotes([]);
    setPastes([]);
    setAttachments([]);
    setFromCanvas(null);
    setResultCard(null);
    setExtraCanvases([]);
    setExtraResultCards([]);
    setAtMenu(null);
    requestAnimationFrame(() => resize());
  }, [attachments, extraCanvases, extraResultCards, fromCanvas, hasDraftContent, pastes, quotes, resize, resultCard, text]);

  /** 输入框空着按 ↑，取回最近清掉的一条。 */
  const recallCleared = useCallback(() => {
    const recalled = cleared[cleared.length - 1];
    if (!recalled) return false;
    touchedRef.current = true;
    changedDuringSendRef.current = true;
    setCleared((previous) => previous.slice(0, -1));
    setText(recalled.text);
    setQuotes(recalled.quotes ?? []);
    setPastes(recalled.pastes);
    setAttachments(recalled.attachments);
    setFromCanvas(recalled.fromCanvas);
    setResultCard(recalled.resultCard);
    setExtraCanvases(recalled.extraCanvases);
    setExtraResultCards(recalled.extraResultCards);
    requestAnimationFrame(() => {
      resize();
      const input = inputRef.current;
      if (!input) return;
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    });
    return true;
  }, [cleared, resize]);

  /** 恢复对话时把原文放回输入框（字接在已有字后面；附带的东西按顺序加在后面）。 */
  const putBack = useCallback(
    (source: RewindSource) => {
      touchedRef.current = true;
      changedDuringSendRef.current = true;
      if (source.text) {
        setText((previous) =>
          !previous.trim()
            ? source.text
            : previous.replace(/\s+$/, '') + String.fromCharCode(10) + source.text,
        );
      }
      if (source.pastes.length > 0) setPastes((previous) => previous.concat(source.pastes));
      if (source.files.length > 0) {
        setAttachments((previous) =>
          previous.concat(
            source.files.map((file) => ({
              id: file.id,
              name: file.name,
              bytes: file.bytes,
              mime: file.mime ?? mimeFromFileName(file.name) ?? 'text/plain',
            })),
          ),
        );
      }
      if (source.resultCards.length > 0) {
        if (resultCard === null) {
          setResultCard(source.resultCards[0] ?? null);
          if (source.resultCards.length > 1) {
            setExtraResultCards((previous) => previous.concat(source.resultCards.slice(1)));
          }
        } else {
          setExtraResultCards((previous) => previous.concat(source.resultCards));
        }
      }
      if (source.canvases.length > 0) {
        if (fromCanvas === null) {
          setFromCanvas(source.canvases[0] ?? null);
          if (source.canvases.length > 1) {
            setExtraCanvases((previous) => previous.concat(source.canvases.slice(1)));
          }
        } else {
          setExtraCanvases((previous) => previous.concat(source.canvases));
        }
      }
      requestAnimationFrame(() => {
        resize();
        inputRef.current?.focus();
      });
    },
    [fromCanvas, resize, resultCard],
  );

  useImperativeHandle(
    ref,
    () => ({
      submit: () => void submit(),
      focus: () => inputRef.current?.focus(),
      // T015.4：画布里的按钮把一句话放进输入框（不替用户发）：已经写了的留着，另起一行接上。
      putFromCanvas: (value, canvas) => {
        touchedRef.current = true;
        changedDuringSendRef.current = true;
        setText((previous) => (previous.trim() ? previous.replace(/\s+$/, '') + String.fromCharCode(10) + value : value));
        setFromCanvas(canvas);
        requestAnimationFrame(() => {
          resize();
          inputRef.current?.focus();
        });
      },
      hasDraftContent,
      clearToHistory,
      recallCleared,
      putBack,
      putQuote: (quote) => {
        touchedRef.current = true;
        changedDuringSendRef.current = true;
        setQuotes((previous) => previous.concat(quote));
        requestAnimationFrame(() => inputRef.current?.focus());
      },
    }),
    [clearToHistory, hasDraftContent, putBack, recallCleared, resize, submit],
  );

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
    if (event.key === 'ArrowUp' && !atMenu) {
      const empty = canRecallCleared({
        hasText: text.trim().length > 0,
        hasPastes: pastes.length > 0,
        hasAttachments: attachments.length > 0,
        hasQuotes: quotes.length > 0,
        hasCanvas: fromCanvas !== null,
        hasResultCard: resultCard !== null,
        hasExtraCanvases: extraCanvases.length > 0,
        hasExtraResultCards: extraResultCards.length > 0,
      });
      if (empty && recallCleared()) {
        event.preventDefault();
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
      const item = atMenu.items[Math.min(atMenu.index, atMenu.items.length - 1)];
      if (item) pickReference(item);
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
        {resultCard ? (
          <ResultCardChip
            title={resultCard.title}
            version={resultCard.version}
            onRemove={() => {
              touchedRef.current = true;
              changedDuringSendRef.current = true;
              setResultCard(null);
            }}
            className="mb-1.5"
          />
        ) : null}
        {extraResultCards.map((card, index) => (
          <ResultCardChip
            key={'extra-result-card-' + index}
            title={card.title}
            version={card.version}
            onRemove={() => setExtraResultCards((previous) => previous.filter((_, position) => position !== index))}
            className="mb-1.5"
          />
        ))}
        {fromCanvas ? (
          <span data-kind="attachment-canvas" className="mb-1.5 block">
            <FromCanvasChip
            title={fromCanvas.title}
            onRemove={() => {
              touchedRef.current = true;
              changedDuringSendRef.current = true;
              setFromCanvas(null);
            }}
              className="mb-1.5"
            />
          </span>
        ) : null}
        {extraCanvases.map((canvas, index) => (
          <span key={'extra-canvas-' + index} data-kind="attachment-canvas" className="mb-1.5 block">
            <FromCanvasChip
              title={canvas.title}
              onRemove={() => setExtraCanvases((previous) => previous.filter((_, position) => position !== index))}
              className="mb-1.5"
            />
          </span>
        ))}
        {quotes.length > 0 || pastes.length > 0 || attachments.length > 0 || objectRefs.length > 0 ? (
          <div ref={cardsRef} className="mb-1.5 flex gap-1.5 overflow-x-auto pb-1" data-kind="composer-cards">
            {quotes.map((item, index) => (
              <QuoteCard
                key={'quote-' + index}
                quote={item.text}
                source={item.source}
                onRemove={() => setQuotes((previous) => previous.filter((_, position) => position !== index))}
              />
            ))}
            {pastes.map((paste, index) => (
              <PasteCard
                key={'paste-' + index}
                paste={paste}
                onOpen={() => setPastePreview(index)}
                onRemove={() => setPastes((previous) => previous.filter((_, item) => item !== index))}
              />
            ))}
            {objectRefs.map((object) => (
              <ObjectRefCard
                key={'object-' + object.kind + '-' + object.id}
                object={object}
                onRemove={() =>
                  setObjectRefs((previous) =>
                    previous.filter((item) => !(item.kind === object.kind && item.id === object.id)),
                  )
                }
              />
            ))}
            {attachments.map((file) =>
              imageMimeSupported(file.mime) ? (
                <ImageCard
                  key={file.id + '-' + file.name}
                  file={file}
                  mime={file.mime}
                  imagesOk={imagesOk ?? modelSeesImages}
                  onRemove={() => setAttachments((previous) => previous.filter((item) => item.id !== file.id))}
                />
              ) : (
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
              ),
            )}
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
            placeholder={
              offline
                ? '连上之后再发…'
                : resultCard
                  ? '要改什么？Kind 会按你说的出一版新要求给你确认'
                  : (placeholder ?? '和 Kind 说…')
            }
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
          {pageTag && !pageTagDismissed ? (
            <div className="mt-1.5 flex items-center" data-kind="composer-page-tag">
              <span className="inline-flex items-center gap-1 rounded-badge border border-line bg-pane px-1.5 py-0.5 text-1 text-ink-2">
                <MapPin className="size-3 text-ink-3" />
                {pageTag}
                <button
                  type="button"
                  aria-label="去掉页面标签"
                  onClick={() => setPageTagDismissed(true)}
                  className="text-ink-3 transition-colors hover:text-ink"
                >
                  <X className="size-3" />
                </button>
              </span>
            </div>
          ) : null}
          <input
            ref={fileInputRef}
            type="file"
            multiple
            hidden
            data-kind="composer-file-input"
            onChange={(event) => {
              const files = Array.from(event.target.files ?? []);
              if (files.length > 0) void intakeFiles(files);
              event.target.value = '';
            }}
          />
          <button
            type="button"
            data-kind="composer-attach"
            aria-label="添加图片或文件"
            title="添加图片或其他文件（也能粘贴/拖进来）"
            onClick={() => fileInputRef.current?.click()}
            className="grid size-7 shrink-0 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink"
          >
            <Paperclip className="size-3.5" />
          </button>
          {children}
        </div>
      </div>
      {quote && !pageQuote ? (
        <button
          type="button"
          data-kind="quote-button"
          onClick={() => {
            const selected = quote.trim();
            if (!selected) return;
            touchedRef.current = true;
            changedDuringSendRef.current = true;
            setQuotes((previous) => [...previous, { text: selected }]);
            setQuote('');
            inputRef.current?.focus();
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
          {atMenu.items.map((item, index) => {
            const group = item.group;
            const previousGroup = index > 0 ? atMenu.items[index - 1].group : null;
            const header =
              group === previousGroup
                ? null
                : group === 'file'
                  ? '文件'
                  : group === 'task'
                    ? '任务'
                    : group === 'report'
                      ? '报告'
                      : '项目';
            const label = group === 'file' ? item.entry.path : item.object.title;
            const key = group === 'file' ? item.entry.path : group + '-' + item.object.id;
            return (
              <div key={key} data-group={group}>
                {header ? (
                  <div data-kind="mention-group" className="px-2 pb-0.5 pt-1 text-1 text-ink-3">
                    {header}
                  </div>
                ) : null}
                <button
                  type="button"
                  data-kind="mention-item"
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
                  <span className={cn('min-w-0 flex-1 truncate', group === 'file' && 'font-mono')}>{label}</span>
                </button>
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
});