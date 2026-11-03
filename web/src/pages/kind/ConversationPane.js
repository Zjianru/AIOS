import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
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
import { ArrowDown, AtSign, CircleHelp, DollarSign, LoaderCircle, MapPin, SendHorizontal, Square, WifiOff } from 'lucide-react';
import { toast } from 'sonner';
import { TurnRow } from './TurnRow';
import { useKindStore } from './kindStore';
import { ModelPicker } from './ModelPicker';
import { useAppStore } from '@/app/store';
import { registerHook } from '@/app/testHooks';
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
    const connectedOnly = useAppStore((state) => state.connectedOnly);
    const running = streamingTurnId !== null;
    const offline = connection.state !== 'online';
    const listRef = useRef(null);
    const inputRef = useRef(null);
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
        if (initialLoading)
            return;
        let cancelled = false;
        let frames = 0;
        let clean = 0;
        let cleanAfterMark = 0;
        const tick = () => {
            if (cancelled || userScrolledRef.current)
                return;
            frames += 1;
            const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
            if (scroller) {
                const gap = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
                if (gap > 2) {
                    scroller.scrollTop = scroller.scrollHeight;
                    clean = 0;
                    cleanAfterMark = 0;
                }
                else if (paintedRef.current) {
                    cleanAfterMark += 1;
                }
                else {
                    clean += 1;
                    if (clean >= 3) {
                        paintedRef.current = true;
                        mark('turns:painted');
                    }
                }
            }
            if (paintedRef.current && (cleanAfterMark > 40 || frames > 180))
                return;
            if (frames > 180)
                return;
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
            const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
            if (scroller)
                scroller.scrollTop = scroller.scrollHeight;
        };
        return useKindStore.subscribe((state, prev) => {
            if (state.views === prev.views)
                return;
            const last = state.views[state.views.length - 1]?.id;
            const prevLast = prev.views[prev.views.length - 1]?.id;
            if (last && last !== prevLast && !state.loadingOlder)
                followRef.current = true;
            if (!followRef.current)
                return;
            requestAnimationFrame(pin);
        });
    }, []);
    // Virtuoso 量到真实高度后总高度还会再长一截；跟着走时内容一变高就再钉一次。
    useEffect(() => {
        if (initialLoading)
            return;
        const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
        const content = scroller?.querySelector('[data-testid="virtuoso-item-list"]');
        if (!scroller || !content)
            return;
        const observer = new ResizeObserver(() => {
            if (followRef.current && paintedRef.current)
                scroller.scrollTop = scroller.scrollHeight;
        });
        observer.observe(content);
        return () => observer.disconnect();
    }, [initialLoading, views.length === 0]);
    const handleAtBottom = useCallback((bottom) => {
        if (bottom)
            followRef.current = true;
        setAtBottom(bottom);
    }, [setAtBottom]);
    const handleRangeChanged = useCallback((range) => {
        if (paintedRef.current || range.startIndex === 0)
            return;
        paintedRef.current = true;
        requestAnimationFrame(() => requestAnimationFrame(() => mark('turns:painted')));
    }, []);
    // Virtuoso 的 startReached 只在「第一条进入可视区」时触发一次，
    // 切完场景重新挂载、再滚到顶它不会再报；所以这里直接盯 scroller 的 scrollTop。
    useEffect(() => {
        if (initialLoading)
            return;
        const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
        if (!scroller)
            return;
        const onScroll = () => {
            if (scroller.scrollTop > 24)
                return;
            const state = useKindStore.getState();
            if (state.initialLoading || !state.hasOlder || state.loadingOlder)
                return;
            void state.loadOlder();
        };
        scroller.addEventListener('scroll', onScroll, { passive: true });
        return () => scroller.removeEventListener('scroll', onScroll);
    }, [initialLoading]);
    const handleStartReached = useCallback(() => {
        const state = useKindStore.getState();
        if (state.initialLoading || !userScrolledRef.current)
            return;
        void state.loadOlder();
    }, []);
    const scrollToLatest = useCallback(() => {
        followRef.current = true;
        listRef.current?.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'smooth' });
    }, []);
    useEffect(() => {
        registerHook('scrollToTop', (() => {
            const scroller = document.querySelector('[data-virtuoso-scroller="true"]');
            if (scroller)
                scroller.scrollTop = 0;
        }));
        registerHook('scrollToBottom', (() => {
            listRef.current?.scrollToIndex({ index: 'LAST', align: 'end' });
        }));
        registerHook('sendMessage', ((text, totalMs) => {
            useKindStore.getState().send(text, totalMs);
        }));
        registerHook('cardIndexes', (() => {
            const indexes = {};
            useKindStore.getState().views.forEach((view, index) => {
                for (const card of view.cards) {
                    if (indexes[card.type] === undefined)
                        indexes[card.type] = index;
                }
            });
            return indexes;
        }));
        registerHook('scrollToFraction', ((fraction) => {
            const scroller = document.querySelector('[data-pane="conversation"] [data-virtuoso-scroller="true"]');
            if (!scroller)
                return false;
            scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) * Math.min(1, Math.max(0, fraction));
            return true;
        }));
        // 注意：Virtuoso 的 scrollToIndex 收的是 data 里的下标，不是 firstItemIndex 之后的绝对下标。
        registerHook('scrollToDataIndex', ((index) => {
            listRef.current?.scrollToIndex({ index, align: 'start' });
        }));
        registerHook('loadAll', (async () => {
            let loaded = 0;
            for (;;) {
                const state = useKindStore.getState();
                if (!state.hasOlder)
                    break;
                const before = state.views.length;
                await state.loadOlder();
                loaded += useKindStore.getState().views.length - before;
            }
            return loaded;
        }));
        registerHook('conversationState', (() => {
            const state = useKindStore.getState();
            return { count: state.views.length, atBottom: state.atBottom, streaming: state.streamingTurnId };
        }));
    }, []);
    // 停止（D12）：运行中按 Esc 随时停；有弹层开着时让弹层先处理 Esc。
    useEffect(() => {
        if (!running)
            return;
        const onKey = (event) => {
            if (event.key !== 'Escape' || event.defaultPrevented)
                return;
            if (document.querySelector('[role="dialog"],[role="menu"]'))
                return;
            event.preventDefault();
            stop();
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [running, stop]);
    const handleSend = useCallback(() => {
        const input = inputRef.current;
        if (!input)
            return;
        const text = input.value.trim();
        if (!text)
            return;
        if (useKindStore.getState().connection.state !== 'online')
            return;
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
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col bg-content", "data-pane": "conversation", onWheelCapture: (event) => {
            userScrolledRef.current = true;
            if (event.deltaY < 0)
                followRef.current = false;
        }, onTouchStartCapture: () => {
            userScrolledRef.current = true;
        }, onKeyDownCapture: () => {
            userScrolledRef.current = true;
        }, children: [offline ? (_jsxs("div", { className: "flex shrink-0 items-center gap-2 border-b border-wait/30 bg-wait-soft px-6 py-2 text-2 text-wait", "data-kind": "offline", children: [_jsx(WifiOff, { className: "size-3.5" }), "\u548C\u5185\u6838\u7684\u8FDE\u63A5\u65AD\u4E86\uFF0C\u6B63\u5728\u91CD\u8FDE\uFF08\u7B2C ", _jsx("span", { className: "brand-type", children: connection.attempt }), " \u6B21\uFF09", _jsx("span", { className: "text-ink-3", children: "\u00B7 \u65AD\u5F00\u671F\u95F4\u7684\u66F4\u65B0\u8FDE\u4E0A\u540E\u4F1A\u8865\u9F50\uFF0C\u4E0D\u4F1A\u4E22\uFF1B\u73B0\u5728\u5148\u4E0D\u80FD\u53D1\u6D88\u606F" })] })) : null, _jsxs("div", { className: "relative min-h-0 flex-1", children: [initialLoading ? (_jsx("div", { className: "flex h-full items-center justify-center text-3 text-ink-3", children: "\u6B63\u5728\u53D6\u6700\u540E\u4E00\u9875\u2026" })) : views.length === 0 ? (_jsx("div", { className: "flex h-full items-center justify-center px-6", "data-kind": "empty", children: _jsxs("div", { className: "max-w-[420px] text-center", children: [_jsx("span", { className: "mx-auto block size-2.5 rounded-full bg-brand" }), _jsx("div", { className: "mt-3 text-4 leading-6 text-ink", children: "\u8FD9\u91CC\u662F\u4F60\u548C Kind \u7684\u5BF9\u8BDD" }), _jsx("p", { className: "mt-1 text-2 leading-5 text-ink-3", children: "\u8BF4\u8BF4\u4F60\u60F3\u505A\u4EC0\u4E48\u3002\u8FD9\u6761\u5BF9\u8BDD\u4E00\u76F4\u7559\u7740\uFF0C\u4E0B\u6B21\u6253\u5F00\u8FD8\u56DE\u5230\u8FD9\u91CC\u3002" })] }) })) : (_jsx(Virtuoso, { ref: listRef, data: views, firstItemIndex: firstItemIndex, initialTopMostItemIndex: { index: firstItemIndex + views.length - 1, align: 'end' }, "data-conversation": "true", startReached: handleStartReached, atBottomStateChange: handleAtBottom, atBottomThreshold: 8, rangeChanged: handleRangeChanged, followOutput: (isAtBottom) => (isAtBottom ? 'auto' : false), increaseViewportBy: { top: 800, bottom: 400 }, itemContent: (_index, view) => (_jsx("div", { className: "mx-auto w-full max-w-[680px]", children: _jsx(TurnRow, { view: view, dayLabel: dayLabels.get(view.id) ?? null, showLastSeen: lastSeenId === view.id, lastSeenCounts: lastSeenCounts }) })) })), _jsx(AnimatePresence, { children: !atBottom && !initialLoading ? (_jsxs(m.button, { type: "button", onClick: scrollToLatest, initial: { opacity: 0, y: 8 }, animate: { opacity: 1, y: 0 }, exit: { opacity: 0, y: 8 }, transition: { duration: 0.12, ease: [0.2, 0.8, 0.2, 1] }, className: "absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-line-2 bg-raised px-3 py-1.5 text-2 text-ink-2 shadow-[var(--shadow-pop)] hover:text-ink", children: [_jsx(ArrowDown, { className: "size-3.5" }), "\u56DE\u5230\u6700\u65B0"] })) : null })] }), _jsx("div", { className: "shrink-0 border-t border-line px-6 pb-2 pt-3", children: _jsxs("div", { className: "mx-auto w-full max-w-[680px]", children: [_jsxs("div", { className: cn('rounded-card border border-line-2 bg-raised/70 px-3 py-2 transition-colors'), children: [connectedOnly ? null : (_jsxs("div", { className: "mb-1.5 flex min-h-[18px] items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: "opacity-70", children: "\u8FD9\u6761\u6D88\u606F\u4F1A\u5E26\u4E0A\u7684\u4E1C\u897F" }), _jsxs("span", { className: "flex items-center gap-0.5 rounded-badge bg-hover px-1 py-[1px]", children: [_jsx(AtSign, { className: "size-3" }), "\u89D2\u8272"] }), _jsxs("span", { className: "flex items-center gap-0.5 rounded-badge bg-hover px-1 py-[1px]", children: [_jsx(DollarSign, { className: "size-3" }), "\u80FD\u529B"] })] })), _jsxs("div", { className: "flex items-end gap-2", children: [_jsx("textarea", { ref: inputRef, rows: 1, placeholder: offline ? '连上之后再发…' : '和 Kind 说…', onInput: (event) => setCanSend(event.currentTarget.value.trim().length > 0), onKeyDown: (event) => {
                                                if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                                                    event.preventDefault();
                                                    handleSend();
                                                }
                                            }, className: "max-h-32 min-h-6 flex-1 resize-none bg-transparent text-4 leading-6 text-ink outline-none placeholder:text-ink-3" }), connectedOnly ? null : (_jsxs("button", { type: "button", onClick: () => toast('「指一下」（批注）排在第 3 期，第 1 期先不做', { description: '现在可以先在对话里直接说，或者用 @ 角色 / $ 能力 引用' }), title: "\u6307\u4E00\u4E0B\uFF08\u6279\u6CE8\uFF09\u5728\u7B2C 3 \u671F", className: "flex h-7 items-center gap-1 rounded-btn px-2 text-2 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2", children: [_jsx(MapPin, { className: "size-3.5" }), "\u6307\u4E00\u4E0B"] })), running ? (_jsx("button", { type: "button", onClick: stop, title: "\u505C\u6B62\uFF08Esc\uFF09\uFF1A\u968F\u65F6\u6253\u65AD Kind \u6B63\u5728\u505A\u7684\u8FD9\u4E00\u8F6E", "aria-label": "\u505C\u6B62", "data-kind": "stop", className: "grid size-7 place-items-center rounded-full bg-ink text-content transition-opacity hover:opacity-90", children: _jsx(Square, { className: "size-3 fill-current" }) })) : (_jsx("button", { type: "button", onClick: handleSend, title: offline ? '连上之后再发' : canSend ? '发送' : '先写点什么', "aria-disabled": !canSend || offline, className: 'grid size-7 place-items-center rounded-full transition-opacity ' +
                                                (canSend && !offline ? 'bg-ink text-content hover:opacity-90' : 'bg-hover text-ink-3'), children: _jsx(SendHorizontal, { className: "size-3.5" }) }))] })] }), _jsxs("div", { className: "group/status mt-1.5 flex items-center gap-2 text-1 text-ink-3", children: [_jsx(ModelPicker, {}), compacting ? (_jsxs("span", { className: "flex items-center gap-1.5 text-ink-2", "data-kind": "compacting", children: [_jsx(LoaderCircle, { className: "size-3 animate-spin" }), "\u6B63\u5728\u538B\u7F29\u4E0A\u4E0B\u6587\u2026"] })) : (_jsxs("span", { className: "hidden items-center gap-2 group-hover/status:flex group-focus-within/status:flex", children: [_jsx("span", { className: "opacity-60", children: "\u00B7" }), _jsxs("span", { children: ["\u5360\u7528 ", _jsxs("span", { className: "brand-type", children: [usage, "%"] })] }), _jsx("button", { type: "button", onClick: compact, disabled: running || usage === 0, title: running ? '这一轮做完再压缩' : '把之前的对话换成一份摘要，腾出上下文；原文留在日志里', className: "rounded-btn px-1 py-[1px] text-ink-2 transition-colors hover:bg-hover hover:text-ink disabled:cursor-not-allowed disabled:opacity-50", "data-kind": "compact", children: "\u538B\u7F29" }), connectedOnly ? null : (_jsxs(_Fragment, { children: [_jsx("span", { className: "opacity-60", children: "\u00B7" }), _jsxs("span", { children: ["\u672C\u6B21\u82B1\u8D39 ", _jsx("span", { className: "brand-type", children: "\u00A53.29" })] })] }))] })), waitingQuestion ? (_jsxs("button", { type: "button", onClick: scrollToLatest, className: "ml-auto flex items-center gap-1 text-wait transition-opacity hover:opacity-80", "data-kind": "waiting-hint", children: [_jsx(CircleHelp, { className: "size-3" }), "Kind \u5728\u7B49\u4F60\u56DE\u7B54"] })) : (_jsx("span", { className: "ml-auto opacity-70", children: "\u539F\u578B\uFF1AKind \u7684\u56DE\u7B54\u662F\u5199\u597D\u7684\u6837\u4F8B" }))] }), _jsx("p", { className: "mt-1 text-1 leading-4 text-ink-3", children: connectedOnly
                                ? 'AIOS 概念 · Kind：你和 AIOS 的一条长对话，一直留着；停止随时可用（Esc）。'
                                : 'AIOS 概念 · Kind：一个对话 + 一个右栏，不做页签；右栏上半是「要你看的和在动的」，下半是它现在带着的上下文。' })] }) })] }));
}
