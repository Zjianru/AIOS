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
import { ArrowDown, CircleHelp, LoaderCircle, SendHorizontal, Square, WifiOff } from 'lucide-react';
import { toast } from 'sonner';
import { TurnRow } from './TurnRow';
import { useKindStore } from './kindStore';
import { ModelPicker } from './ModelPicker';
import { mark } from '@/lib/perf';
import { cn } from '@/lib/utils';

export function ConversationPane() {
  const views = useKindStore((state) => state.views);
  const firstItemIndex = useKindStore((state) => state.firstItemIndex);
  const dayLabels = useKindStore((state) => state.dayLabels);
  const lastSeenId = useKindStore((state) => state.lastSeenId);
  const lastSeenCounts = useKindStore((state) => state.lastSeenCounts);
  const atBottom = useKindStore((state) => state.atBottom);
  const initialLoading = useKindStore((state) => state.initialLoading);
  const loadInitial = useKindStore((state) => state.loadInitial);
  const setAtBottom = useKindStore((state) => state.setAtBottom);
  const send = useKindStore((state) => state.send);
  const stop = useKindStore((state) => state.stop);
  const compact = useKindStore((state) => state.compact);
  const streamingTurnId = useKindStore((state) => state.streamingTurnId);
  const connection = useKindStore((state) => state.connection);
  const usage = useKindStore((state) => state.usage);
  const compacting = useKindStore((state) => state.compacting);
  const waitingQuestion = useKindStore((state) => {
    const last = state.views[state.views.length - 1];
    return last?.question?.status === 'pending';
  });
  const running = streamingTurnId !== null;
  const offline = connection.state !== 'online';
  const listRef = useRef<VirtuosoHandle | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const [canSend, setCanSend] = useState(false);
  const paintedRef = useRef(false);
  // 首次用户滚动之前不自动往上取页：5 轮比视口短时 Virtuoso 会立刻触发 startReached。
  const userScrolledRef = useRef(false);
  /** 跟着最新走：贴着底时为真；往上翻就放开，回到底或者新起一轮再接上。 */
  const followRef = useRef(true);

  useEffect(() => {
    void loadInitial();
  }, [loadInitial]);

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

  const handleSend = useCallback(() => {
    const input = inputRef.current;
    if (!input) return;
    const text = input.value.trim();
    if (!text) return;
    if (useKindStore.getState().connection.state !== 'online') return;
    if (useKindStore.getState().streamingTurnId) {
      toast('Kind 还在做上一轮', { description: '等它做完，或者先停下（Esc）' });
      return;
    }
    input.value = '';
    setCanSend(false);
    followRef.current = true;
    send(text);
    listRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' });
  }, [send]);

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
            rangeChanged={handleRangeChanged}
            followOutput={(isAtBottom: boolean) => (isAtBottom ? 'auto' : false)}
            increaseViewportBy={{ top: 800, bottom: 400 }}
            itemContent={(_index, view) => (
              <div className="mx-auto w-full max-w-[680px]">
                <TurnRow
                  view={view}
                  dayLabel={dayLabels.get(view.id) ?? null}
                  showLastSeen={lastSeenId === view.id}
                  lastSeenCounts={lastSeenCounts}
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
            
            <div className="flex items-end gap-2">
              <textarea
                ref={inputRef}
                rows={1}
                placeholder={offline ? '连上之后再发…' : '和 Kind 说…'}
                onInput={(event) => setCanSend(event.currentTarget.value.trim().length > 0)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                    event.preventDefault();
                    handleSend();
                  }
                }}
                className="max-h-32 min-h-6 flex-1 resize-none bg-transparent text-4 leading-6 text-ink outline-none placeholder:text-ink-3"
              />
              
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
                  className={
                    'grid size-7 place-items-center rounded-full transition-opacity ' +
                    (canSend && !offline ? 'bg-ink text-content hover:opacity-90' : 'bg-hover text-ink-3')
                  }
                >
                  <SendHorizontal className="size-3.5" />
                </button>
              )}
            </div>
          </div>
          <div className="group/status mt-1.5 flex items-center gap-2 text-1 text-ink-3">
            <ModelPicker />
            {/* 信息分层（brief §四.3）：底栏只留模型名，占用、压缩和花费鼠标悬停才出；正在压缩时一直显示 */}
            {compacting ? (
              <span className="flex items-center gap-1.5 text-ink-2" data-kind="compacting">
                <LoaderCircle className="size-3 animate-spin" />
                正在压缩上下文…
              </span>
            ) : (
              <span className="hidden items-center gap-2 group-hover/status:flex group-focus-within/status:flex">
                <span className="opacity-60">·</span>
                <span>
                  占用 <span className="brand-type">{usage}%</span>
                </span>
                <button
                  type="button"
                  onClick={compact}
                  disabled={running || usage === 0}
                  title={running ? '这一轮做完再压缩' : '把之前的对话换成一份摘要，腾出上下文；原文留在日志里'}
                  className="rounded-btn px-1 py-[1px] text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
                  data-kind="compact"
                >
                  压缩
                </button>
                
              </span>
            )}
            {waitingQuestion ? (
              <button
                type="button"
                onClick={scrollToLatest}
                className="ml-auto flex items-center gap-1 text-wait transition-opacity hover:opacity-80"
                data-kind="waiting-hint"
              >
                <CircleHelp className="size-3" />
                Kind 在等你回答
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}
