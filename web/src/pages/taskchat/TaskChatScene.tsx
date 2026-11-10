/**
 * 任务对话（#taskchat:<taskId>）：一个任务的 ThinAgent 会话只读快照。
 *
 * - 头部是任务事实（TaskRead 的 task：标题、要求、DoD、cwd、冻结的模型、Attempt 数）。
 * - 轮次复用 Kind 对话的 TurnRow；第一条 assignment 是程序消息，不画成用户气泡。
 * - 本票没有输入框、不能发送、不能停止；不连 /ws，翻页用 task-read 的 beforeSeq。
 * - 会话日志缺失/损坏时给固定的降级说明，原因照服务给的事实写，页面不编。
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, ChevronRight, FolderOpen } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { displayPath, ServiceHttpError, taskRead } from '@/data/service';
import type { ServiceEvent } from '@/data/service';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import { TurnRow } from '@/pages/kind/TurnRow';
import { deriveTurnViews } from '@/pages/kind/turnViews';
import { cn } from '@/lib/utils';

const NO_CANVAS = () => {};
const NO_COUNTS = { moved: 0, waiting: 0 };

/** 任务会话里的第一条用户消息是执行者写的 assignment；它不算用户气泡。 */
const ASSIGNMENT_PREFIX = 'assignment:';

const THIN_TOOLS = new Set([
  'read',
  'glob',
  'grep',
  'skill',
  'todo_write',
  'web_search',
  'web_fetch',
  'task_report',
]);

function rpcIdOf(event: ServiceEvent): string | null {
  const message = event.data?.message;
  if (!message || typeof message !== 'object') return null;
  const source = (message as { source?: unknown }).source;
  if (!source || typeof source !== 'object') return null;
  const typed = source as { kind?: unknown; rpcId?: unknown };
  if (typed.kind !== 'user' || typeof typed.rpcId !== 'string') return null;
  return typed.rpcId;
}

/** 页面上不显示的 assignment 事件；其余事件照日志。 */
function visibleTaskEvents(events: ServiceEvent[]): ServiceEvent[] {
  return events.filter(
    (event) => event.type !== 'user/message' || !rpcIdOf(event)?.startsWith(ASSIGNMENT_PREFIX),
  );
}

/** 服务错误 → 给人看的原因；task/unavailable 的正文里已经是可读事实。 */
function reasonOf(error: unknown): string {
  if (error instanceof ServiceHttpError && error.detail) return error.detail;
  if (error instanceof Error && error.message) return error.message;
  return '未知原因';
}

function toolsText(tools: string[]): string {
  if (tools.length === THIN_TOOLS.size && tools.every((tool) => THIN_TOOLS.has(tool))) {
    return '只读：读、搜文件、能力、计划、搜网页、读网页、上报';
  }
  return tools.join('、');
}

function Brief({ task }: { task: TaskReadHeader }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-2">
      <button
        type="button"
        data-kind="task-brief-toggle"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
        任务要求和 DoD
      </button>
      {open ? (
        <div
          className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-l border-line pl-3 text-2 leading-5"
          data-kind="task-brief"
        >
          <span className="text-ink-3">要求</span>
          <span className="text-ink-2">{task.requirement}</span>
          <span className="text-ink-3">DoD</span>
          <span className="text-ink-2">{task.dod}</span>
          <span className="text-ink-3">模型</span>
          <span className="text-ink-2">{task.model}（派出时定下，不跟着改）</span>
          <span className="text-ink-3">工具</span>
          <span className="text-ink-2">{toolsText(task.tools)}</span>
          <span className="text-ink-3">Attempt</span>
          <span className="text-ink-2">第 {task.attempts} 次</span>
        </div>
      ) : null}
    </div>
  );
}

function BackToKind() {
  return (
    <button
      type="button"
      onClick={() => go('kind')}
      className="-ml-1 flex items-center gap-1 rounded-btn px-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
    >
      <ArrowLeft className="size-3.5" />
      回 Kind 对话
    </button>
  );
}

export function TaskChatScene() {
  const taskId = useRouteStore((state) => state.route.id);
  const [phase, setPhase] = useState<'loading' | 'ready' | 'error'>('loading');
  const [task, setTask] = useState<TaskReadHeader | null>(null);
  const [events, setEvents] = useState<ServiceEvent[]>([]);
  const [error, setError] = useState('');
  const [hasOlder, setHasOlder] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const requestToken = useRef(0);
  const bottomPinned = useRef(false);
  const loadingOlderRef = useRef(false);
  const restoreRef = useRef<{ height: number; top: number } | null>(null);

  useEffect(() => {
    const token = ++requestToken.current;
    setPhase('loading');
    setTask(null);
    setEvents([]);
    setError('');
    setHasOlder(false);
    setLoadingOlder(false);
    loadingOlderRef.current = false;
    bottomPinned.current = false;
    restoreRef.current = null;
    if (!taskId) {
      setError('地址里没有任务 id');
      setPhase('error');
      return;
    }
    void taskRead(taskId)
      .then((response) => {
        if (token !== requestToken.current) return;
        setTask(response.task);
        setEvents(response.events);
        setHasOlder(response.events.length > 0);
        setPhase('ready');
      })
      .catch((reason: unknown) => {
        if (token !== requestToken.current) return;
        setError(reasonOf(reason));
        setPhase('error');
      });
  }, [taskId]);

  // 首屏落到最新一条。
  useLayoutEffect(() => {
    if (phase !== 'ready' || bottomPinned.current) return;
    bottomPinned.current = true;
    const scroller = scrollRef.current;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [phase]);

  // 往上翻：新页插在前面，保持原来的阅读位置。
  useLayoutEffect(() => {
    const restore = restoreRef.current;
    if (!restore) return;
    restoreRef.current = null;
    const scroller = scrollRef.current;
    if (!scroller) return;
    scroller.scrollTop = scroller.scrollHeight - restore.height + restore.top;
  }, [events]);

  // 终态/Waiting 报告之后执行者会立刻关掉会话，日志可能没有 turn/end；
  // 头部带来的 reportedAt 是「这次 Attempt 交过报告」的事实，据此把最后一轮收成已完成。
  const views = useMemo(() => {
    const derived = deriveTurnViews(visibleTaskEvents(events));
    const reportedAt = task?.reportedAt;
    if (reportedAt === undefined || reportedAt === null || derived.length === 0) return derived;
    const last = derived.length - 1;
    if (derived[last].status !== 'inProgress') return derived;
    const startedAt = derived[last].startedAt;
    const durationMs = startedAt === null ? null : Math.max(0, reportedAt - startedAt * 1000);
    return derived.map((view, index) =>
      index === last ? { ...view, status: 'completed' as const, durationMs } : view,
    );
  }, [events, task]);

  async function loadOlder() {
    if (loadingOlderRef.current || !hasOlder || events.length === 0) return;
    const token = requestToken.current;
    const scroller = scrollRef.current;
    const before = { height: scroller?.scrollHeight ?? 0, top: scroller?.scrollTop ?? 0 };
    const oldestSeq = events[0].seq;
    loadingOlderRef.current = true;
    setLoadingOlder(true);
    try {
      const response = await taskRead(taskId, oldestSeq);
      if (token !== requestToken.current) return;
      if (response.events.length === 0) {
        setHasOlder(false);
        return;
      }
      restoreRef.current = before;
      setEvents((current) => [...response.events, ...current]);
    } catch (reason) {
      if (token === requestToken.current) {
        toast('更早的记录没读出来', { description: reasonOf(reason) });
      }
    } finally {
      if (token === requestToken.current) {
        loadingOlderRef.current = false;
        setLoadingOlder(false);
      }
    }
  }

  function onScroll() {
    const scroller = scrollRef.current;
    if (!scroller || phase !== 'ready' || scroller.scrollTop > 64) return;
    void loadOlder();
  }

  if (phase === 'loading') {
    return (
      <div
        className="flex h-full items-center justify-center bg-content px-6"
        data-pane="task-conversation"
      >
        <div className="text-2 text-ink-3" data-kind="task-loading">
          正在打开任务对话…
        </div>
      </div>
    );
  }

  if (phase === 'error' || task === null) {
    return (
      <div
        className="flex h-full items-center justify-center bg-content px-6"
        data-pane="task-conversation"
      >
        <div
          className="w-full max-w-[460px] rounded-card border border-line-2 bg-raised/60 p-4"
          data-kind="task-read-fallback"
        >
          <div className="text-4 font-medium leading-6 text-ink">任务对话打不开</div>
          <p className="mt-1.5 text-2 leading-5 text-ink-2">
            这个任务的对话现在打不开。回到 Kind 对话，稍后再点一次结果卡。
          </p>
          <p className="mt-2 break-words text-2 leading-5 text-ink-3" data-kind="task-read-reason">
            原因：{error || '未知原因'}
          </p>
          <div className="mt-3">
            <BackToKind />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-content" data-pane="task-conversation">
      <div className="shrink-0 border-b border-line px-6 py-3">
        <div className="mx-auto w-full max-w-[680px]">
          <BackToKind />
          <div className="mt-2 flex items-start gap-3">
            <h1
              className="min-w-0 flex-1 text-4 font-medium leading-6 text-ink"
              data-kind="task-full-title"
            >
              {task.title}
            </h1>
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-2 text-ink-3">
            <span>调查任务 · 只读</span>
            <span className="flex items-center gap-1">
              <FolderOpen className="size-3.5" />
              <span className="process-path">{displayPath(task.cwd)}</span>
            </span>
            <span>ThinAgent · {task.model}</span>
          </div>
          <Brief task={task} />
        </div>
      </div>

      <div
        className="min-h-0 flex-1 overflow-y-auto"
        data-kind="task-scroll"
        ref={scrollRef}
        onScroll={onScroll}
      >
        <div className="mx-auto w-full max-w-[680px] pb-6" data-conversation="task">
          {views.length === 0 ? (
            <div className="px-6 py-10 text-center text-3 leading-6 text-ink-3" data-kind="task-empty">
              这个任务的会话还没写下任何内容。
            </div>
          ) : (
            views.map((view) => (
              <TurnRow
                key={view.id}
                view={view}
                dayLabel={null}
                showLastSeen={false}
                lastSeenCounts={NO_COUNTS}
                actor="ThinAgent"
                readOnly
                cwd={task.cwd}
                onCanvasMessage={NO_CANVAS}
              />
            ))
          )}
          {loadingOlder ? (
            <div className="px-6 py-2 text-center text-1 text-ink-3" data-kind="task-loading-older">
              正在读更早的记录…
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
