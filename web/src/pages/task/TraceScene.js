import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 原始记录（第三层，#trace:<taskId>；§4.3；brief §四「任务详情」）。
 *
 * - 子助手、工具调用、模型请求、文件改动、审查结论按时间顺序列出；顶部写明「这里是第三层，平时不需要看」；
 * - 500 轮样例：打开只取最后一页（20 轮，desc），往上滚到顶再取更早的（20 轮/页），保持阅读位置；
 * - 按参与者筛选（只筛已加载的部分，页脚写明）；
 * - 虚拟列表用 react-virtuoso；打开到画完打 trace:painted，测速量这一段。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { Virtuoso } from 'react-virtuoso';
import { ArrowLeft, ChevronDown } from 'lucide-react';
import { TASK_RUN_TOTAL_TURNS, taskRunResume, taskRunTurns } from '@/data/api';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { mark } from '@/lib/perf';
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger, } from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';
const PAGE_INITIAL = 20;
const PAGE_OLDER = 20;
const FIRST_INDEX_BASE = 1_000_000;
function clockOf(seconds) {
    if (!seconds)
        return '—';
    const date = new Date(seconds * 1000);
    return (String(date.getHours()).padStart(2, '0') +
        ':' +
        String(date.getMinutes()).padStart(2, '0') +
        ':' +
        String(date.getSeconds()).padStart(2, '0'));
}
function buildRow(turn) {
    let participant = '—';
    let kind = '事件';
    let what = '';
    let command;
    for (const item of turn.items) {
        if (item.type === 'agentMessage') {
            const parts = item.text.split(' · ');
            participant = parts[0] ?? '—';
            kind = parts[1] ?? '事件';
            what = parts.slice(2).join(' · ');
        }
        if (item.type === 'commandExecution') {
            command = {
                command: item.command,
                output: item.aggregatedOutput ?? '',
                exitCode: item.exitCode,
                durationMs: item.durationMs,
            };
        }
    }
    return { id: turn.id, time: clockOf(turn.startedAt), participant, kind, what, status: turn.status, command };
}
const STATUS_TONE = {
    completed: 'text-ink-3',
    inProgress: 'text-run',
    failed: 'text-fail',
    interrupted: 'text-wait',
};
export function TraceScene() {
    const taskId = useRouteStore((state) => state.route.id);
    const [rows, setRows] = useState([]);
    const [firstItemIndex, setFirstItemIndex] = useState(FIRST_INDEX_BASE);
    const [hasOlder, setHasOlder] = useState(false);
    const [loading, setLoading] = useState(true);
    const [loadingOlder, setLoadingOlder] = useState(false);
    const [filter, setFilter] = useState('all');
    const stateRef = useRef({ nextCursor: null, hasOlder: false, loadingOlder: false });
    useEffect(() => {
        let alive = true;
        setRows([]);
        setLoading(true);
        stateRef.current = { nextCursor: null, hasOlder: false, loadingOlder: false };
        void (async () => {
            const response = await taskRunResume(PAGE_INITIAL);
            if (!alive)
                return;
            const page = response.initialTurnsPage;
            const initial = page.data.slice().reverse().map(buildRow);
            stateRef.current.nextCursor = page.nextCursor;
            stateRef.current.hasOlder = Boolean(page.nextCursor);
            setRows(initial);
            setFirstItemIndex(FIRST_INDEX_BASE - initial.length);
            setHasOlder(Boolean(page.nextCursor));
            setLoading(false);
            requestAnimationFrame(() => requestAnimationFrame(() => mark('trace:painted')));
        })();
        return () => {
            alive = false;
        };
    }, [taskId]);
    const loadOlder = useCallback(async () => {
        const state = stateRef.current;
        if (!state.hasOlder || state.loadingOlder || !state.nextCursor)
            return;
        state.loadingOlder = true;
        setLoadingOlder(true);
        try {
            const response = await taskRunTurns({ cursor: state.nextCursor, limit: PAGE_OLDER });
            const older = response.data.slice().reverse().map(buildRow);
            setRows((current) => older.concat(current));
            setFirstItemIndex((index) => index - older.length);
            state.nextCursor = response.nextCursor;
            state.hasOlder = Boolean(response.nextCursor);
            setHasOlder(Boolean(response.nextCursor));
        }
        finally {
            state.loadingOlder = false;
            setLoadingOlder(false);
        }
    }, []);
    const participants = ['all'].concat([...new Set(rows.map((row) => row.participant))]);
    const visible = filter === 'all' ? rows : rows.filter((row) => row.participant === filter);
    const total = TASK_RUN_TOTAL_TURNS;
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col bg-content", "data-scene-page": "trace", children: [_jsxs("header", { className: "flex h-11 shrink-0 items-center gap-2 border-b border-line px-4 text-3", children: [_jsxs("button", { type: "button", onClick: () => go('task:' + taskId), className: "flex items-center gap-1 rounded-btn px-1.5 py-1 text-2 text-ink-3 transition-colors hover:bg-hover hover:text-ink", children: [_jsx(ArrowLeft, { className: "size-3.5" }), "\u4EFB\u52A1"] }), _jsx("span", { className: "brand-type text-2 text-ink-2", children: taskId }), _jsx("span", { className: "text-1 text-ink-3", children: "\u00B7 \u8FD9\u91CC\u662F\u7B2C\u4E09\u5C42\uFF0C\u5E73\u65F6\u4E0D\u9700\u8981\u770B\u3002" }), _jsx("span", { className: "ml-auto" }), _jsxs(DropdownMenu, { children: [_jsx(DropdownMenuTrigger, { asChild: true, children: _jsxs("button", { type: "button", className: "flex h-7 items-center gap-1 rounded-btn px-2 text-2 text-ink-2 transition-colors hover:bg-hover", children: [filter === 'all' ? '全部参与者' : filter, " ", _jsx(ChevronDown, { className: "size-3.5" })] }) }), _jsx(DropdownMenuContent, { align: "end", className: "w-48", children: _jsxs(DropdownMenuRadioGroup, { value: filter, onValueChange: setFilter, children: [_jsx(DropdownMenuRadioItem, { value: "all", children: "\u5168\u90E8\u53C2\u4E0E\u8005" }), participants
                                            .filter((name) => name !== 'all')
                                            .map((name) => (_jsx(DropdownMenuRadioItem, { value: name, children: name }, name)))] }) })] }), _jsxs("span", { className: "brand-type text-1 text-ink-3", children: ["\u5DF2\u52A0\u8F7D ", loading ? 0 : rows.length, " / ", total, " \u6761"] })] }), _jsx("div", { className: "min-h-0 flex-1", children: loading ? (_jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u8FD0\u884C\u8BB0\u5F55\u2026" })) : (_jsx(Virtuoso, { style: { height: '100%' }, data: visible, firstItemIndex: firstItemIndex, startReached: loadOlder, overscan: 600, itemContent: (_index, row) => (_jsxs("div", { className: "flex items-start gap-2 border-b border-line/50 px-4 py-[7px] text-1", "data-trace-row": "true", children: [_jsx("span", { className: "brand-type w-[58px] shrink-0 text-ink-3", children: row.time }), _jsx("span", { className: "w-[92px] shrink-0 truncate text-ink-2", children: row.participant }), _jsx("span", { className: "w-[58px] shrink-0 text-ink-3", children: row.kind }), _jsxs("span", { className: "min-w-0 flex-1", children: [_jsx("span", { className: cn('text-ink-2', STATUS_TONE[row.status]), children: row.what || '—' }), row.command ? (_jsxs("code", { className: "mt-0.5 block truncate font-mono text-ink-3", children: [row.command.command, row.command.durationMs ? ' · ' + (row.command.durationMs / 1000).toFixed(1) + 's' : ''] })) : null] })] })) })) }), _jsxs("footer", { className: "flex shrink-0 items-center gap-2 border-t border-line px-4 py-2 text-1 text-ink-3", children: [_jsx("span", { children: loadingOlder ? '正在取更早的记录…' : hasOlder ? '往上滚加载更早的记录' : '已经是全部记录' }), _jsx("span", { className: "ml-auto", children: "\u7B5B\u9009\u53EA\u4F5C\u7528\u4E8E\u5DF2\u52A0\u8F7D\u7684\u90E8\u5206\uFF1B\u539F\u578B\u91CC\u8FD0\u884C\u8BB0\u5F55\u662F 500 \u8F6E\u6837\u4F8B\u3002" })] }), _jsx("div", { className: "shrink-0 border-t border-line px-4 py-1.5 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u539F\u59CB\u8BB0\u5F55\uFF08\u7B2C\u4E09\u5C42\uFF09\uFF1A\u5B50\u52A9\u624B\u3001\u5DE5\u5177\u8C03\u7528\u3001\u6A21\u578B\u8BF7\u6C42\u3001\u6587\u4EF6\u6539\u52A8\u3001\u5BA1\u67E5\u7ED3\u8BBA\u6309\u65F6\u95F4\u5217\u5728\u8FD9\u91CC\uFF0C\u5E73\u65F6\u4E0D\u9700\u8981\u770B\u3002" })] }));
}
