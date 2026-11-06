/**
 * 对话面板（需求 §三.3）：
 * - 正文居中，最宽 680（layout-motion-v1.md §二）；
 * - 2000 轮：打开只取最后一页 5 轮；往上滚到顶再取 20 轮，保持阅读位置；
 * - 在底部时新消息自动跟随；不在底部时不跟，显示「回到最新」；
 * - 流式期间只重绘最后一条（见 TurnRow 里的 Answer）。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { AnimatePresence, m } from 'motion/react';
import { Virtuoso } from 'react-virtuoso';
import type { ListRange, VirtuosoHandle } from 'react-virtuoso';
import { ArrowDown, SendHorizontal, Square, WifiOff } from 'lucide-react';
import { toast } from 'sonner';
import { TurnRow } from './TurnRow';
import { useKindStore } from './kindStore';
import { ModelPicker } from './ModelPicker';
import { ComposerDock } from './ComposerDock';
import { Composer } from './composer/Composer';
import type { ComposerHandle } from './composer/Composer';
import { mark } from '@/lib/perf';
import { cn } from '@/lib/utils';

/** T013.8：阅读位置（照 DSH capturePosition：顶行 key + 偏移；找不到这一行用 scrollTop）。 */
interface ReadingPosition {
  /** 顶上那一行的轮次 id（视图里的 key）。 */
  turnId: string | null;
  /** 这一行在视口里的偏移（像素）。 */
  offset: number;
  /** 兜底：原始 scrollTop。 */
  scrollTop: number;
}

const readingPositions = new Map<string, ReadingPosition>();

export function ConversationPane() {
  const views = useKindStore((state) => state.views);
  const firstItemIndex = useKindStore((state) => state.firstItemIndex);
  const dayLabels = useKindStore((state) => state.dayLabels);
  const lastSeenId = useKindStore((state) => state.lastSeenId);
  const lastSeenCounts = useKindStore((state) => state.lastSeenCounts);
  const atBottom = useKindStore((state) => state.atBottom);
  const initialLoading = useKindStore((state) => state.initialLoading);
  const setAtBottom = useKindStore((state) => state.setAtBottom);
  const send = useKindStore((state) => state.send);
  const stop = useKindStore((state) => state.stop);
  const streamingTurnId = useKindStore((state) => state.streamingTurnId);
  const connection = useKindStore((state) => state.connection);
  const waitingQuestion = useKindStore((state) => {
    const last = state.views[state.views.length - 1];
    return last?.question?.status === 'pending';
  });
  const running = streamingTurnId !== null;
  const offline = connection.state !== 'online';
  const sessionId = useKindStore((state) => state.sessionId);
  const scrollerRef = useRef<HTMLElement | null>(null);
  const topIndexRef = useRef<number | null>(null);
  const capturedForRef = useRef<string | null>(null);
  const restoreRef = useRef<ReadingPosition | null>(null);
  const listRef = useRef<VirtuosoHandle | null>(null);
  // T013.8：历史加载完再把阅读位置恢复回去（列表布局稳定后）。
  useEffect(() => {
    if (initialLoading) return;
    const saved = restoreRef.current;
    if (!saved) return;
    let cancelled = false;
    const restore = () => {
      if (cancelled) return;
      const scroller = scrollerRef.current;
      if (!scroller) return;
      const viewIndex = saved.turnId ? views.findIndex((view) => view.id === saved.turnId) : -1;
      if (viewIndex >= 0) {
        listRef.current?.scrollToIndex({ index: firstItemIndex + viewIndex, offset: -saved.offset, behavior: 'auto' });
      } else {
        scroller.scrollTop = saved.scrollTop;
      }
      // 只恢复一次：之后的新消息/滚动不再把用户拉回旧位置。
      restoreRef.current = null;
    };
    requestAnimationFrame(() => requestAnimationFrame(restore));
    const timer = setTimeout(restore, 400);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [firstItemIndex, initialLoading, views]);
  const [canSend, setCanSend] = useState(false);
  const paintedRef = useRef(false);
  // 首次用户滚动之前不自动往上取页：5 轮比视口短时 Virtuoso 会立刻触发 startReached。
  const userScrolledRef = useRef(false);
  /** 跟着最新走：贴着底时为真；往上翻就放开，回到底或者新起一轮再接上。 */
  const followRef = useRef(true);


  // 首屏：落到最新一条。
  // Virtuoso 先按估算高度排版，量到真实高度后它的滚动模型会比 DOM 少一截（这里差不多 66px），
  // 所以除了走它的 API，还要在 rAF 里直接盯着 scroller 把位置钉到底；
  // 连续 3 帧贴住底就打 mark（这就是测速量的「最后一页画完」），之后继续钉一会儿直到稳定。
  useEffect(() => {
    if (initialLoading) return;
    let cancelled = false;
    let frames = 0;
    let clean = 0;
    let cleanAfterMark = 0;
    const tick = () => {
      if (cancelled || userScrolledRef.current) return;
      frames += 1;
      const scroller = document.querySelector<HTMLElement>(
        '[data-pane="conversation"] [data-virtuoso-scroller="true"]',
      );
      if (scroller) {
        const gap = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
        if (gap > 2) {
          scroller.scrollTop = scroller.scrollHeight;
          clean = 0;
          cleanAfterMark = 0;
        } else if (paintedRef.current) {
          cleanAfterMark += 1;
        } else {
          clean += 1;
          if (clean >= 3) {
            paintedRef.current = true;
            mark('turns:painted');
          }
        }
      }
      if (paintedRef.current && (cleanAfterMark > 40 || frames > 180)) return;
      if (frames > 180) return;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return () => {
      cancelled = true;
    };
  }, [initialLoading]);

  // 一轮在长（工具一条条出、回答流式写）时，最后一条变高不会触发 Virtuoso 的 followOutput，
  // 所以盯着 store：跟着走时每次变化都钉到底；新起一轮（最后一条换了）总是跳过去。
  useEffect(() => {
    const pin = () => {
      const scroller = document.querySelector<HTMLElement>('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    };
    return useKindStore.subscribe((state, prev) => {
      if (state.views === prev.views) return;
      const last = state.views[state.views.length - 1]?.id;
      const prevLast = prev.views[prev.views.length - 1]?.id;
      if (last && last !== prevLast && !state.loadingOlder) followRef.current = true;
      if (!followRef.current) return;
      requestAnimationFrame(pin);
    });
  }, []);

  // Virtuoso 量到真实高度后总高度还会再长一截；跟着走时内容一变高就再钉一次。
  useEffect(() => {
    if (initialLoading) return;
    const scroller = document.querySelector<HTMLElement>('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
    const content = scroller?.querySelector('[data-testid="virtuoso-item-list"]');
    if (!scroller || !content) return;
    const observer = new ResizeObserver(() => {
      if (followRef.current && paintedRef.current) scroller.scrollTop = scroller.scrollHeight;
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, [initialLoading, views.length === 0]);

  const handleAtBottom = useCallback(
    (bottom: boolean) => {
      if (bottom) followRef.current = true;
      setAtBottom(bottom);
    },
    [setAtBottom],
  );

  const handleRangeChanged = useCallback((range: ListRange) => {
    if (paintedRef.current || range.startIndex === 0) return;
    paintedRef.current = true;
    requestAnimationFrame(() => requestAnimationFrame(() => mark('turns:painted')));
  }, []);

  // Virtuoso 的 startReached 只在「第一条进入可视区」时触发一次，
  // 切完场景重新挂载、再滚到顶它不会再报；所以这里直接盯 scroller 的 scrollTop。
  useEffect(() => {
    if (initialLoading) return;
    const scroller = document.querySelector<HTMLElement>(
      '[data-pane="conversation"] [data-virtuoso-scroller="true"]',
    );
    if (!scroller) return;
    const onScroll = () => {
      if (scroller.scrollTop > 24) return;
      if (!userScrolledRef.current) return;
      const state = useKindStore.getState();
      if (state.initialLoading || !state.hasOlder || state.loadingOlder) return;
      void state.loadOlder();
    };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => scroller.removeEventListener('scroll', onScroll);
  }, [initialLoading]);

  const handleStartReached = useCallback(() => {
    const state = useKindStore.getState();
    if (state.initialLoading || !userScrolledRef.current) return;
    void state.loadOlder();
  }, []);

  const scrollToLatest = useCallback(() => {
    followRef.current = true;
    listRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'smooth' });
  }, []);

  // T015.2：点了旧版画布那一行 —— 滚到出最新版的那一轮。
  const reveal = useKindStore((state) => state.reveal);
  useEffect(() => {
    if (!reveal) return;
    const index = useKindStore.getState().views.findIndex((view) => view.id === reveal.turnId);
    if (index < 0) return;
    followRef.current = false;
    listRef.current?.scrollToIndex({
      index: firstItemIndex + index,
      align: 'start',
      behavior: 'smooth',
    });
  }, [reveal, firstItemIndex]);


  // 停止（D12）：运行中按 Esc 随时停；有弹层开着时让弹层先处理 Esc。
  useEffect(() => {
    if (!running) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      if (document.querySelector('[role="dialog"],[role="menu"]')) return;
      event.preventDefault();
      stop();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [running, stop]);

  const composerRef = useRef<ComposerHandle | null>(null);

  // T015.4：画布里的按钮把一句话交给输入框（只放进输入框，由用户自己发）。
  const onCanvasMessage = useCallback(
    (text: string, canvas: { id: string; title: string }) => {
      composerRef.current?.putFromCanvas(text, canvas);
    },
    [],
  );

  // T013.7：发送走 Composer（Enter/Cmd+Enter/按钮都到 submit）；pastes/attachments 跟着发。
  const handleSend = useCallback(() => {
    if (useKindStore.getState().connection.state !== 'online') return;
    if (useKindStore.getState().streamingTurnId) {
      toast('Kind 还在做上一轮', { description: '等它做完，或者先停下（Esc）' });
      return;
    }
    composerRef.current?.submit();
  }, []);

  const onComposerSend = useCallback(
    (text: string, options: { pastes: string[]; attachments: { id: string; name: string; bytes: number }[] }) => {
      followRef.current = true;
      listRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' });
      return send(text, options);
    },
    [send],
  );

  return (
    <div
      className="flex h-full min-h-0 flex-col bg-content"
      data-pane="conversation"
      onWheelCapture={(event) => {
        userScrolledRef.current = true;
        if (event.deltaY < 0) followRef.current = false;
      }}
      onTouchStartCapture={() => {
        userScrolledRef.current = true;
      }}
      onKeyDownCapture={() => {
        userScrolledRef.current = true;
      }}
    >
      {offline ? (
        <div className="flex shrink-0 items-center gap-2 border-b border-wait/30 bg-wait-soft px-6 py-2 text-2 text-wait" data-kind="offline">
          <WifiOff className="size-3.5" />
          和内核的连接断了，正在重连（第 <span className="brand-type">{connection.attempt}</span> 次）
          <span className="text-ink-3">· 断开期间的更新连上后会补齐，不会丢；现在先不能发消息</span>
        </div>
      ) : null}
      <div className="relative min-h-0 flex-1">
        {initialLoading ? (
          <div className="flex h-full items-center justify-center text-3 text-ink-3">正在取最后一页…</div>
        ) : views.length === 0 ? (
          <div className="flex h-full items-center justify-center px-6" data-kind="empty">
            <div className="max-w-[420px] text-center">
              <span className="mx-auto block size-2.5 rounded-full bg-brand" />
              <div className="mt-3 text-4 leading-6 text-ink">这里是你和 Kind 的对话</div>
              <p className="mt-1 text-2 leading-5 text-ink-3">说说你想做什么。这条对话一直留着，下次打开还回到这里。</p>
            </div>
          </div>
        ) : (
          <Virtuoso

            ref={listRef}
            data={views}
            firstItemIndex={firstItemIndex}
            initialTopMostItemIndex={{ index: firstItemIndex + views.length - 1, align: 'end' }}
            data-conversation="true"
            startReached={handleStartReached}
            atBottomStateChange={handleAtBottom}
            atBottomThreshold={8}
            rangeChanged={(range: ListRange) => {
              topIndexRef.current = range.startIndex;
              handleRangeChanged(range);
            }}
            followOutput={(isAtBottom: boolean) => (isAtBottom ? 'auto' : false)}
            increaseViewportBy={{ top: 800, bottom: 400 }}
            scrollerRef={(ref) => {
              const element = ref as HTMLElement | null;
              scrollerRef.current = element;
              if (!element || !sessionId) return;
              if (capturedForRef.current === sessionId) return;
              capturedForRef.current = sessionId;
              const saved = readingPositions.get(sessionId) ?? null;
              restoreRef.current = saved;
            }}
            onScroll={(event: React.UIEvent<HTMLElement>) => {
              const element = event.currentTarget;
              if (!sessionId || !element) return;
              // rangeChanged 是异步的；这里用最近一次的值（onScroll 连续触发，误差可忽略）。
              const topIndex = topIndexRef.current;
              const view = topIndex === null ? undefined : views[topIndex - firstItemIndex];
              // 顶行相对视口顶的偏移（capturePosition 的 offset）。
              let offset = 0;
              if (topIndex !== null) {
                const row = element.querySelector('[data-item-index="' + String(topIndex) + '"]');
                if (row) {
                  offset = row.getBoundingClientRect().top - element.getBoundingClientRect().top;
                }
              }
              readingPositions.set(sessionId, {
                turnId: view?.id ?? null,
                offset,
                scrollTop: element.scrollTop,
              });
            }}
            itemContent={(_index, view) => (
              <div className="mx-auto w-full max-w-[680px]">
                <TurnRow
                  view={view}
                  dayLabel={dayLabels.get(view.id) ?? null}
                  showLastSeen={lastSeenId === view.id}
                  lastSeenCounts={lastSeenCounts}
                  onCanvasMessage={onCanvasMessage}
                />
              </div>
            )}
          />
        )}

        <AnimatePresence>
          {!atBottom && !initialLoading ? (
            <m.button
              type="button"
              onClick={scrollToLatest}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: 8 }}
              transition={{ duration: 0.12, ease: [0.2, 0.8, 0.2, 1] }}
              className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-line-2 bg-raised px-3 py-1.5 text-2 text-ink-2 shadow-[var(--shadow-pop)] hover:text-ink"
            >
              <ArrowDown className="size-3.5" />
              回到最新
            </m.button>
          ) : null}
        </AnimatePresence>
      </div>

      <div className="shrink-0 border-t border-line px-6 pb-2 pt-3">
        <div className="mx-auto w-full max-w-[680px]">
          <div className={cn('rounded-card border border-line-2 bg-raised/70 px-3 py-2 transition-colors')}>
            
            <Composer
              ref={composerRef}
              offline={offline}
              onSend={onComposerSend}
              onCanSendChange={setCanSend}
            >
              <ModelPicker />
              {running ? (
                <button
                  type="button"
                  onClick={stop}
                  title="停止（Esc）：随时打断 Kind 正在做的这一轮"
                  aria-label="停止"
                  data-kind="stop"
                  className="grid size-7 place-items-center rounded-full bg-ink text-content transition-opacity hover:opacity-90"
                >
                  <Square className="size-3 fill-current" />
                </button>
              ) : (
                <button
                  type="button"
                  onClick={handleSend}
                  title={offline ? '连上之后再发' : canSend ? '发送' : '先写点什么'}
                  aria-disabled={!canSend || offline}
                  data-kind="send"
                  className={
                    'grid size-7 place-items-center rounded-full transition-opacity ' +
                    (canSend && !offline ? 'bg-ink text-content hover:opacity-90' : 'bg-hover text-ink-3')
                  }
                >
                  <SendHorizontal className="size-3.5" />
                </button>
              )}
            </Composer>
          </div>
          <ComposerDock
            running={running}
            waitingQuestion={waitingQuestion}
            onJumpToQuestion={scrollToLatest}
          />
        </div>
      </div>
    </div>
  );
}
