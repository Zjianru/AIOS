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
import type { Turn, TurnStatus } from '@/data/types';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { mark } from '@/lib/perf';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';

const PAGE_INITIAL = 20;
const PAGE_OLDER = 20;
const FIRST_INDEX_BASE = 1_000_000;

interface TraceRow {
  id: string;
  time: string;
  participant: string;
  kind: string;
  what: string;
  status: TurnStatus;
  command?: { command: string; output: string; exitCode: number | null; durationMs: number | null };
}

function clockOf(seconds: number | null): string {
  if (!seconds) return '—';
  const date = new Date(seconds * 1000);
  return (
    String(date.getHours()).padStart(2, '0') +
    ':' +
    String(date.getMinutes()).padStart(2, '0') +
    ':' +
    String(date.getSeconds()).padStart(2, '0')
  );
}

function buildRow(turn: Turn): TraceRow {
  let participant = '—';
  let kind = '事件';
  let what = '';
  let command: TraceRow['command'];
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

const STATUS_TONE: Record<TurnStatus, string> = {
  completed: 'text-ink-3',
  inProgress: 'text-run',
  failed: 'text-fail',
  interrupted: 'text-wait',
};

export function TraceScene() {
  const taskId = useRouteStore((state) => state.route.id);
  const [rows, setRows] = useState<TraceRow[]>([]);
  const [firstItemIndex, setFirstItemIndex] = useState(FIRST_INDEX_BASE);
  const [hasOlder, setHasOlder] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [filter, setFilter] = useState('all');
  const stateRef = useRef({ nextCursor: null as string | null, hasOlder: false, loadingOlder: false });

  useEffect(() => {
    let alive = true;
    setRows([]);
    setLoading(true);
    stateRef.current = { nextCursor: null, hasOlder: false, loadingOlder: false };
    void (async () => {
      const response = await taskRunResume(PAGE_INITIAL);
      if (!alive) return;
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
    if (!state.hasOlder || state.loadingOlder || !state.nextCursor) return;
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
    } finally {
      state.loadingOlder = false;
      setLoadingOlder(false);
    }
  }, []);

  const participants = ['all'].concat([...new Set(rows.map((row) => row.participant))]);
  const visible = filter === 'all' ? rows : rows.filter((row) => row.participant === filter);
  const total = TASK_RUN_TOTAL_TURNS;

  return (
    <div className="flex h-full min-h-0 flex-col bg-content" data-scene-page="trace">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-line px-4 text-3">
        <button
          type="button"
          onClick={() => go('task:' + taskId)}
          className="flex items-center gap-1 rounded-btn px-1.5 py-1 text-2 text-ink-3 transition-colors hover:bg-hover hover:text-ink"
        >
          <ArrowLeft className="size-3.5" />
          任务
        </button>
        <span className="brand-type text-2 text-ink-2">{taskId}</span>
        <span className="text-1 text-ink-3">· 这里是第三层，平时不需要看。</span>

        <span className="ml-auto" />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="flex h-7 items-center gap-1 rounded-btn px-2 text-2 text-ink-2 transition-colors hover:bg-hover"
            >
              {filter === 'all' ? '全部参与者' : filter} <ChevronDown className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuRadioGroup value={filter} onValueChange={setFilter}>
              <DropdownMenuRadioItem value="all">全部参与者</DropdownMenuRadioItem>
              {participants
                .filter((name) => name !== 'all')
                .map((name) => (
                  <DropdownMenuRadioItem key={name} value={name}>
                    {name}
                  </DropdownMenuRadioItem>
                ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
        <span className="brand-type text-1 text-ink-3">
          已加载 {loading ? 0 : rows.length} / {total} 条
        </span>
      </header>

      <div className="min-h-0 flex-1">
        {loading ? (
          <div className="grid h-full place-items-center text-2 text-ink-3">正在取运行记录…</div>
        ) : (
          <Virtuoso
            style={{ height: '100%' }}
            data={visible}
            firstItemIndex={firstItemIndex}
            startReached={loadOlder}
            overscan={600}
            itemContent={(_index, row) => (
              <div className="flex items-start gap-2 border-b border-line/50 px-4 py-[7px] text-1" data-trace-row="true">
                <span className="brand-type w-[58px] shrink-0 text-ink-3">{row.time}</span>
                <span className="w-[92px] shrink-0 truncate text-ink-2">{row.participant}</span>
                <span className="w-[58px] shrink-0 text-ink-3">{row.kind}</span>
                <span className="min-w-0 flex-1">
                  <span className={cn('text-ink-2', STATUS_TONE[row.status])}>{row.what || '—'}</span>
                  {row.command ? (
                    <code className="mt-0.5 block truncate font-mono text-ink-3">
                      {row.command.command}
                      {row.command.durationMs ? ' · ' + (row.command.durationMs / 1000).toFixed(1) + 's' : ''}
                    </code>
                  ) : null}
                </span>
              </div>
            )}
          />
        )}
      </div>

      <footer className="flex shrink-0 items-center gap-2 border-t border-line px-4 py-2 text-1 text-ink-3">
        <span>{loadingOlder ? '正在取更早的记录…' : hasOlder ? '往上滚加载更早的记录' : '已经是全部记录'}</span>
        <span className="ml-auto">筛选只作用于已加载的部分；原型里运行记录是 500 轮样例。</span>
      </footer>
    </div>
  );
}
