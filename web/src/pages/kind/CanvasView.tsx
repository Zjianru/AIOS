/**
 * 画布（T015.2；D58，样子照原型 app-v14 a938d2e 的 CanvasView.tsx）。
 *
 * - 最新版展开：标题栏「画布 · 标题」，能收起、重新载入、切版本；内容超过 420px 给「展开」。
 * - 旧版那一轮缩成一行（CanvasStub），点了跳到出最新版的那一轮。
 * - 切版本只在页面上切（shown 是本地状态）；日志和发给模型的仍是最新版。
 * - 正文是沙箱 iframe：src="/canvas-frame"，sandbox 只给脚本和表单（不给同源、弹窗、下载）。
 * - 父窗口把这一版的 HTML 用 postMessage 交给外壳，外壳 document.write 写进自己的文档；
 *   外壳报尺寸，父窗口把主题变量交给它（MCP Apps 的标准名），深浅色跟着 AIOS。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { AppWindow, ChevronDown, ChevronRight, Maximize2, Minimize2, RotateCcw } from 'lucide-react';
import { useAppStore } from '@/app/store';
import { cn } from '@/lib/utils';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import type { CanvasVersionView, CanvasViewDoc } from './turnViews';

/** 不展开时最多这么高；超过了画布里面滚动，标题栏给「展开」。 */
const MAX_HEIGHT = 420;

/**
 * AIOS 的设计变量换成 MCP Apps 的标准名（照原型 hostVariables）。
 * 只给画布要的那几样；值从页面上取，深浅色切换时重新发一次。
 */
function hostVariables(): Record<string, string> {
  const css = getComputedStyle(document.documentElement);
  const value = (name: string) => css.getPropertyValue(name).trim();
  return {
    '--color-background-primary': value('--raised'),
    '--color-background-secondary': value('--pane'),
    '--color-background-inverse': value('--ink'),
    '--color-text-primary': value('--ink'),
    '--color-text-secondary': value('--ink-2'),
    '--color-text-tertiary': value('--ink-3'),
    '--color-text-inverse': value('--content'),
    '--color-text-danger': value('--fail'),
    '--color-text-success': value('--done'),
    '--color-border-primary': value('--line'),
    '--color-border-secondary': value('--line-2'),
    '--font-sans': value('--font-sans'),
    '--font-mono': value('--font-mono'),
    '--font-weight-normal': '400',
    '--font-weight-medium': '500',
    '--font-weight-semibold': '600',
    '--font-text-sm-size': '12px',
    '--font-text-md-size': '14px',
    '--font-heading-lg-size': '24px',
    '--border-radius-sm': value('--r-btn'),
    '--border-radius-md': value('--r-card'),
  };
}

/**
 * 画布转过来的滚轮：滚外面那个滚动区，再在 iframe 上补一个滚轮事件，
 * 让对话知道是你在滚（往上滚就不再钉到底）。补的这个事件本身不会滚动。
 */
function forwardWheel(frame: HTMLIFrameElement | null, dy: number, dx: number) {
  if (!frame) return;
  frame.dispatchEvent(
    new WheelEvent('wheel', { deltaX: dx, deltaY: dy, bubbles: true, cancelable: true }),
  );
  let node: HTMLElement | null = frame.parentElement;
  while (node && !(node.scrollHeight > node.clientHeight && /(auto|scroll)/.test(getComputedStyle(node).overflowY))) {
    node = node.parentElement;
  }
  node?.scrollBy({ top: dy, left: 0 });
}

interface FrameProps {
  html: string;
  /** 展开时不限高。 */
  expanded: boolean;
  /** T015.4：画布里的按钮交上来的一句话（放进输入框，不发送）。 */
  onMessage: (text: string) => void;
  onOverflow: (overflow: boolean) => void;
}

function CanvasFrame({ html, expanded, onMessage, onOverflow }: FrameProps) {
  const ref = useRef<HTMLIFrameElement | null>(null);
  const [height, setHeight] = useState(64);
  const theme = useAppStore((state) => state.theme);
  // 外壳每次重新载入（换 key）只收一次 render：document.write 之后又会来一次 load。
  const rendered = useRef(false);

  const sendHost = useCallback(() => {
    // 等外壳把 data-theme 换好再取值（子组件的 effect 比外壳的先跑）
    requestAnimationFrame(() => {
      ref.current?.contentWindow?.postMessage(
        { aiosHost: true, theme, variables: hostVariables() },
        '*',
      );
    });
  }, [theme]);

  // 主题变了重新发一次变量（MCP Apps host-context-changed）
  useEffect(() => {
    sendHost();
  }, [sendHost]);

  const onLoad = useCallback(() => {
    const frame = ref.current?.contentWindow;
    if (!frame || rendered.current) return;
    rendered.current = true;
    frame.postMessage({ aiosHost: true, type: 'render', html }, '*');
  }, [html]);

  useEffect(() => {
    const listen = (event: MessageEvent) => {
      // 只认自己这个 iframe 发来的（sandbox 没给同源，按 source 认）
      if (event.source !== ref.current?.contentWindow) return;
      const data = event.data as {
        aiosCanvas?: boolean;
        type?: string;
        height?: number;
        text?: string;
        dx?: number;
        dy?: number;
      } | null;
      if (!data?.aiosCanvas) return;
      if (data.type === 'ready') sendHost();
      if (data.type === 'size' && typeof data.height === 'number') setHeight(data.height);
      if (data.type === 'message' && typeof data.text === 'string') onMessage(data.text);
      if (data.type === 'wheel' && typeof data.dy === 'number') {
        forwardWheel(ref.current, data.dy, typeof data.dx === 'number' ? data.dx : 0);
      }
    };
    window.addEventListener('message', listen);
    return () => window.removeEventListener('message', listen);
  }, [onMessage, sendHost]);

  const overflow = height > MAX_HEIGHT;
  useEffect(() => onOverflow(overflow), [overflow, onOverflow]);

  return (
    <iframe
      ref={ref}
      title="画布"
      sandbox="allow-scripts allow-forms"
      src="/canvas-frame"
      onLoad={onLoad}
      className="block w-full border-0 bg-transparent"
      style={{ height: expanded ? height : Math.min(height, MAX_HEIGHT) }}
      data-kind="canvas-frame"
    />
  );
}

interface CanvasViewProps {
  /** 这一轮出的那一版。 */
  doc: CanvasViewDoc;
  /** 这个画布现在最新是第几版（同一会话里后面的轮可能已经出了新版）。 */
  latest: number;
  /** 这个画布出过的版本，按 n 升序（标题栏切版本用）。 */
  versions: CanvasVersionView[];
  /** 旧版缩成一行时，点它跳到最新那一轮。 */
  onJumpLatest: () => void;
  /** T015.4：画布里的按钮交上来的一句话（带这个画布的 id 和标题）。 */
  onCanvasMessage: (text: string, canvas: { id: string; title: string }) => void;
}

/** 旧版那一轮：缩成一行，点了跳到最新那一轮（照原型 CanvasStub）。 */
function CanvasStub({ doc, latest, onJumpLatest }: Omit<CanvasViewProps, 'versions' | 'onCanvasMessage'>) {
  return (
    <button
      type="button"
      onClick={onJumpLatest}
      className="my-2 flex w-full items-center gap-2 rounded-card border border-line px-3 py-2 text-left text-2 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
      data-kind="canvas-stub"
    >
      <AppWindow className="size-3.5 shrink-0" />
      <span className="shrink-0 text-ink-2">画布 · {doc.title}</span>
      <span className="shrink-0">
        · 第 <span className="brand-type">{doc.n}</span> 版
      </span>
      <span className="min-w-0 truncate">· {doc.note}</span>
      <span className="ml-auto shrink-0">
        已有第 <span className="brand-type">{latest}</span> 版 ›
      </span>
    </button>
  );
}

/** 最新版：标题栏（收起、切版本、重新载入、展开）+ 沙箱 iframe。 */
function LiveCanvas({ doc, latest, versions, onCanvasMessage }: Omit<CanvasViewProps, 'onJumpLatest'>) {
  const [shown, setShown] = useState(latest);
  const [open, setOpen] = useState(true);
  const [expanded, setExpanded] = useState(false);
  const [overflow, setOverflow] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);
  // 新版来了就跳到新版（只在页面上；日志里一直是最新那一版）。
  useEffect(() => setShown(latest), [latest]);
  const available = versions.filter((item) => item.n <= latest);
  const current =
    available.find((item) => item.n === shown) ??
    available[available.length - 1] ?? { n: doc.n, title: doc.title, note: doc.note, html: doc.html };
  // 这一轮展开这个画布（旧版只剩一行）；页面上显示的那一版（可在标题栏切）里的按钮点一下，
  // 就把这句话连同画布的 id/标题交给输入框（不发送）。
  const onMessage = useCallback(
    (text: string) => onCanvasMessage(text, { id: doc.id, title: doc.title }),
    [doc.id, doc.title, onCanvasMessage],
  );

  return (
    <div className="my-3 overflow-hidden rounded-card border border-line-2 bg-raised" data-kind="canvas">
      <div className="flex h-9 items-center gap-1.5 border-b border-line pl-2 pr-1.5 text-2 text-ink-2">
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
          aria-label={open ? '收起画布' : '展开画布'}
          className="flex min-w-0 items-center gap-1.5 rounded-btn px-1 py-0.5 transition-colors hover:text-ink"
        >
          <ChevronRight className={cn('size-3 shrink-0 transition-transform', open && 'rotate-90')} />
          <AppWindow className="size-3.5 shrink-0 text-ink-3" />
          <span className="shrink-0 text-ink-3">画布</span>
          <span className="truncate text-ink" data-kind="canvas-title">
            {current.title}
          </span>
        </button>

        {available.length > 1 ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                data-kind="canvas-versions"
                className="flex shrink-0 items-center gap-0.5 rounded-btn px-1.5 py-0.5 text-ink-3 transition-colors hover:bg-hover hover:text-ink"
              >
                第 <span className="brand-type">{shown}</span> 版{shown === latest ? '' : '（旧）'}
                <ChevronDown className="size-3" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-72">
              <DropdownMenuLabel className="text-1 text-ink-3">
                切到哪一版看（只在你这里切，Kind 那边还是最新版）
              </DropdownMenuLabel>
              {available
                .slice()
                .reverse()
                .map((item) => (
                  <DropdownMenuItem
                    key={item.n}
                    data-kind="canvas-version"
                    data-version={item.n}
                    onSelect={() => setShown(item.n)}
                    className="items-start gap-2"
                  >
                    <span className={cn('brand-type w-10 shrink-0', item.n === shown ? 'text-ink' : 'text-ink-3')}>
                      第 {item.n} 版
                    </span>
                    <span className="text-2 leading-5 text-ink-2">
                      {item.note}
                      {item.n === latest ? <span className="text-ink-3"> · 最新</span> : null}
                    </span>
                  </DropdownMenuItem>
                ))}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}

        <span className="ml-auto flex shrink-0 items-center">
          <button
            type="button"
            aria-label="重新载入"
            title="重新载入（画布里的状态清零）"
            onClick={() => setReloadKey((value) => value + 1)}
            className="rounded-btn p-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink"
          >
            <RotateCcw className="size-3.5" />
          </button>
          {open && (overflow || expanded) ? (
            <button
              type="button"
              aria-label={expanded ? '收起' : '展开'}
              title={expanded ? '收回最大高度' : '按内容全部展开'}
              onClick={() => setExpanded((value) => !value)}
              className="rounded-btn p-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink"
            >
              {expanded ? <Minimize2 className="size-3.5" /> : <Maximize2 className="size-3.5" />}
            </button>
          ) : null}
        </span>
      </div>
      {open ? (
        <CanvasFrame
          // 换版或重新载入都重挂：外壳按新 HTML 重写自己的文档（T015.2；review F6）。
          key={current.n + ':' + String(reloadKey)}
          html={current.html}
          expanded={expanded}
          onMessage={onMessage}
          onOverflow={setOverflow}
        />
      ) : null}
    </div>
  );
}

export function CanvasView({ doc, latest, versions, onJumpLatest, onCanvasMessage }: CanvasViewProps) {
  // 这一轮不是最新版：缩成一行，不占地方（旧版里没有按钮可点，也不交话）。
  if (doc.n < latest) return <CanvasStub doc={doc} latest={latest} onJumpLatest={onJumpLatest} />;
  return (
    <LiveCanvas
      doc={doc}
      latest={latest}
      versions={versions}
      onCanvasMessage={onCanvasMessage}
    />
  );
}
