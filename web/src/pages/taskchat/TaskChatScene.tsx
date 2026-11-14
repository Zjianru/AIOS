/**
 * 任务对话（#taskchat:<taskId>）：一个任务的 ThinAgent 会话只读快照。
 *
 * - 头部是任务事实（TaskRead 的 task：标题、要求、DoD、cwd、冻结的模型、Attempt 数）。
 * - 轮次复用 Kind 对话的 TurnRow；第一条 assignment 是程序消息，不画成用户气泡。
 * - 没有输入框、不能发送、不能停止；打开时 TaskRead 拿头部和第一页，再用 lastSeq 连
 *   `/ws?task=` 收实时帧；翻页用 task-read 的 beforeSeq。
 * - 离开/换任务就关连接，旧连接的帧按 socket 身份丢掉；会话日志缺失/损坏时给固定的
 *   降级说明，原因照服务给的事实写，页面不编。
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowLeft,
  ChevronDown,
  ChevronRight,
  FilePen,
  FolderOpen,
  Hourglass,
  PauseCircle,
  PlugZap,
  SendHorizontal,
  Square,
} from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { useTaskChromeStore } from '@/app/taskChrome';
import {
  connectTaskSession,
  displayPath,
  loadExecSettings,
  ServiceHttpError,
  taskInterrupt,
  taskRead,
  taskSay,
  taskStop,
} from '@/data/service';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import type { Frame, SendAttachmentRef, ServiceEvent } from '@/data/service';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import type { TaskReadLedger } from '@/data/generated/TaskReadLedger';
import type { TaskReadReport } from '@/data/generated/TaskReadReport';
import type { SessionStats } from '@/data/generated/SessionStats';
import type { FxInfo } from '@/data/generated/FxInfo';
import { taskStatusOf } from '@/data/taskResult';
import { turnLabels, useStreamStore } from '@/pages/kind/kindStore';
import { TurnRow, UserCards } from '@/pages/kind/TurnRow';
import { deriveTurnViews } from '@/pages/kind/turnViews';
import type { TurnUserView } from '@/pages/kind/turnViews';
import { Composer } from '@/pages/kind/composer/Composer';
import type { ComposerHandle, ComposerSendOptions } from '@/pages/kind/composer/Composer';
import { mergeLiveEvent, withReportedCompletion } from './taskLive';
import { TaskStatsDock } from './TaskStatsDock';
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
  'task_handback',
  'task_ledger_note',
  'task_ledger_history',
]);

/** 任务对话里说的一句话的 request id 前缀（服务端 task-say:<inputId>）。 */
const TASK_SAY_PREFIX = 'task-say:';

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
    return '只读：读、搜文件、能力、计划、搜网页、读网页、记账本、查账本历史、上报、交回';
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

const NOTE_GROUPS = [
  { kind: 'next', label: '下一步' },
  { kind: 'rejected', label: '否掉的做法' },
] as const;

function clock(at: number): string {
  const date = new Date(at);
  return String(date.getHours()).padStart(2, '0') + ':' + String(date.getMinutes()).padStart(2, '0');
}

/**
 * T056.2：任务页账本（照原型 app-v14 d2a28f2）。
 * 约束是用户原话，进展/待答从最近报告读；叙述条按类分组；关掉的历史点开才看。
 */
function Ledger({ ledger, task }: { ledger: TaskReadLedger; task: TaskReadHeader }) {
  const [open, setOpen] = useState(false);
  const [history, setHistory] = useState(false);
  const constraints = ledger.constraints;
  const live = ledger.notes;
  const closed = [...ledger.history].sort((left, right) => right.closedAt - left.closedAt);
  const report = task.report;
  if (constraints.length === 0 && live.length === 0 && closed.length === 0 && !report) {
    return null;
  }
  const count = constraints.length + live.length + (report ? 1 : 0);
  return (
    <div className="mt-1" data-kind="task-ledger">
      <button
        type="button"
        data-kind="task-ledger-toggle"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
        账本 · {count} 条在用
      </button>
      {open ? (
        <div
          className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-l border-line pl-3 text-2 leading-5"
          data-kind="task-ledger-body"
        >
          {constraints.length ? (
            <>
              <span className="text-ink-3">约束</span>
              <div className="min-w-0 space-y-0.5" data-kind="ledger-constraints">
                {constraints.map((item) => (
                  <div key={item.text} className="text-ink-2">
                    「{item.text}」
                    <span className="text-ink-3">
                      {item.from ? ' · 来自「' + item.from + '」' : ' · 你说的'}
                    </span>
                  </div>
                ))}
              </div>
            </>
          ) : null}
          {report ? (
            <>
              <span className="text-ink-3">进展</span>
              <div className="min-w-0 text-ink-2" data-kind="ledger-progress">
                <span className="line-clamp-2">{report.summary}</span>
                <span className="text-ink-3">
                  第 {Math.max(1, task.attempts)} 次
                  {task.reportedAt != null ? ' · ' + clock(task.reportedAt) + ' 的报告' : ''}
                </span>
              </div>
            </>
          ) : null}
          {report?.question ? (
            <>
              <span className="text-ink-3">待答</span>
              <div className="min-w-0 text-ink-2" data-kind="ledger-question">
                {report.question}
              </div>
            </>
          ) : null}
          {NOTE_GROUPS.map((group) => {
            const notes = live.filter((note) => note.kind === group.kind);
            if (notes.length === 0) return null;
            return (
              <div key={group.kind} className="contents">
                <span className="text-ink-3">{group.label}</span>
                <div className="min-w-0 space-y-0.5" data-kind={'ledger-' + group.kind}>
                  {notes.map((note) => (
                    <div key={note.id} className="text-ink-2">
                      {note.text}
                      <span className="text-ink-3"> · 第 {note.attempt} 次记</span>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
          {closed.length ? (
            <>
              <span />
              <div className="min-w-0">
                <button
                  type="button"
                  onClick={() => setHistory((value) => !value)}
                  className="text-1 text-ink-3 transition-colors hover:text-ink-2"
                  data-kind="ledger-history-toggle"
                >
                  {history ? '收起历史' : '历史 ' + closed.length + ' 条（不再带给它）'}
                </button>
                {history ? (
                  <div className="mt-1 space-y-1" data-kind="ledger-history">
                    {closed.map((note) => (
                      <div key={note.id} className="text-1 leading-4 text-ink-3">
                        <span className="text-ink-2 line-through decoration-ink-4">{note.text}</span>
                        <br />
                        {NOTE_GROUPS.find((group) => group.kind === note.kind)?.label} · 第{' '}
                        {note.attempt} 次记 · {note.closedWhy}，{clock(note.closedAt)}
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** 头部「任务」菜单（D81）：立即停、收尾停；任务没在跑时两项都灰。 */
function TaskMenu({
  running,
  winding,
  onStopNow,
  onStopAfterReport,
}: {
  running: boolean;
  winding: boolean;
  onStopNow: () => void;
  onStopAfterReport: () => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="mt-[1px] flex shrink-0 items-center gap-0.5 rounded-btn border border-line-2 px-2 py-0.5 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
          data-kind="task-menu"
        >
          任务
          <ChevronDown className="size-3" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-[280px]">
        <DropdownMenuItem
          disabled={!running}
          onSelect={onStopNow}
          className="flex-col items-start gap-0"
        >
          <span>立即停</span>
          <span className="text-1 text-ink-3">马上掐断，这次 Attempt 记成已停止</span>
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!running || winding}
          onSelect={onStopAfterReport}
          className="flex-col items-start gap-0"
        >
          <span>收尾停</span>
          <span className="text-1 text-ink-3">做完手上这一步、交一份报告就结束</span>
        </DropdownMenuItem>
        {running ? null : <div className="px-2 py-1.5 text-1 text-ink-3">现在没在跑。</div>}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * 页头状态（T051）：顺序照 spec——在跑 → 执行中（收尾时「执行中 · 收尾」）；已停止；被打断了；
 * 有 report → taskStatusOf(report.state, report.origin, review, accepted)；其余 → 等待中。
 * 词只在 taskStatusOf 一处；旧的笼统说法（交过报告就写待核）不再用。
 */
function TaskState({
  running,
  winding,
  stopped,
  interrupted,
  report,
  review,
  accepted,
}: {
  running: boolean;
  winding: boolean;
  stopped: boolean;
  interrupted: boolean;
  /** 当前 Attempt 最近一份报告的 { state, origin }；null = 还没交报告。 */
  report: TaskReadReport | null;
  /** 写任务当前版本的 ready（和 Kind 卡「待你核」同一判断）。 */
  review: boolean;
  /** 用户接受记录绑的就是当前版本和这份报告。 */
  accepted: boolean;
}) {
  if (running) {
    return (
      <span
        className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-run-soft px-1.5 py-[1px] text-1 text-run"
        data-kind="task-state"
      >
        {winding ? '执行中 · 收尾' : '执行中'}
      </span>
    );
  }
  if (stopped) {
    return (
      <span
        className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-hover px-1.5 py-[1px] text-1 text-ink-2"
        data-kind="task-state"
      >
        <Square className="size-3 shrink-0" />
        已停止
      </span>
    );
  }
  if (interrupted) {
    return (
      <span
        className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-wait-soft px-1.5 py-[1px] text-1 text-wait"
        data-kind="task-state"
      >
        <PlugZap className="size-3 shrink-0" />
        被打断了
      </span>
    );
  }
  if (report !== null) {
    const word = taskStatusOf(
      report.state,
      report.origin,
      review ? 'review' : undefined,
      accepted,
    );
    const failed = word === '失败';
    return (
      <span
        className={
          failed
            ? 'mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-fail-soft px-1.5 py-[1px] text-1 text-fail'
            : 'mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-wait-soft px-1.5 py-[1px] text-1 text-wait'
        }
        data-kind="task-state"
      >
        <Hourglass className="size-3 shrink-0" />
        {word}
      </span>
    );
  }
  // 兜底：任务已派出但还没开跑/在排队时也要有状态词，页头不空白。
  // T051.2 note：词只从 taskStatusOf 一处出（pending 派发态就是「等待中」），这里不再写字面量。
  return (
    <span
      className="mt-[3px] inline-flex shrink-0 items-center gap-1 rounded-badge bg-hover px-1.5 py-[1px] text-1 text-ink-2"
      data-kind="task-state"
    >
      {taskStatusOf('pending', 'executor')}
    </span>
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
  const setTaskTitle = useTaskChromeStore((state) => state.setTaskTitle);
  const [phase, setPhase] = useState<'loading' | 'ready' | 'error'>('loading');
  const [task, setTask] = useState<TaskReadHeader | null>(null);
  const [ledger, setLedger] = useState<TaskReadLedger | null>(null);
  // T051：统计由服务端 task-read 给（和 Kind status 同一个 session_stats），页面只显示。
  const [attemptStats, setAttemptStats] = useState<SessionStats | null>(null);
  const [fx, setFx] = useState<FxInfo | null>(null);
  const [currency, setCurrency] = useState<string | null>(null);
  const [winding, setWinding] = useState(false);
  const [events, setEvents] = useState<ServiceEvent[]>([]);
  const [error, setError] = useState('');
  const [hasOlder, setHasOlder] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  /** T028.5：全局暂停（横幅；消息先排着）。 */
  const [paused, setPaused] = useState(false);
  /** T028.5：说的是不是正在送；送的时候按钮不重复触发。 */
  const [saying, setSaying] = useState(false);
  /** T028.5：已经受理、还没出现在会话日志里的输入（排队或正在送）。 */
  const [queued, setQueued] = useState<
    { inputId: string; text: string; pastes: string[]; attachments: SendAttachmentRef[] }[]
  >([]);
  const composerRef = useRef<ComposerHandle | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const requestToken = useRef(0);
  /** T039：页头事实请求的序号——断线重连可能连拉几次，只认最新一次的回包。 */
  const headerRequest = useRef(0);
  const bottomPinned = useRef(false);
  const loadingOlderRef = useRef(false);
  /** 到顶时上一页还没装好：装好后按新的位置再判一次，不把这次到顶丢掉。 */
  const pendingOlderRef = useRef(false);
  const restoreRef = useRef<{ height: number; top: number } | null>(null);
  const socketRef = useRef<WebSocket | null>(null);
  /** 主动断开时等 onclose（放开服务端的会话写者）。 */
  const socketClosedRef = useRef<(() => void) | null>(null);
  /** 当前的连接函数（停下来以后说一句话：先断、发完再接）。 */
  const connectRef = useRef<(() => void) | null>(null);
  const lastSeqRef = useRef(0);
  const liveTurnRef = useRef<number | null>(null);
  const streamTextRef = useRef('');
  const followBottomRef = useRef(true);

  useEffect(() => {
    const token = ++requestToken.current;
    let disposed = false;
    let reconnectAttempt = 0;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

    setPhase('loading');
    setTask(null);
    setLedger(null);
    // 标题要等 TaskRead 的事实；加载前先清掉，免得还带着上一个任务的名字。
    setTaskTitle(null);
    setWinding(false);
    setEvents([]);
    setError('');
    setHasOlder(false);
    setLoadingOlder(false);
    setQueued([]);
    loadingOlderRef.current = false;
    pendingOlderRef.current = false;
    bottomPinned.current = false;
    restoreRef.current = null;
    followBottomRef.current = true;
    lastSeqRef.current = 0;
    liveTurnRef.current = null;
    streamTextRef.current = '';
    const previous = socketRef.current;
    socketRef.current = null;
    previous?.close();
    useStreamStore.setState({ turnId: null, itemId: null, text: '' });

    if (!taskId) {
      setError('地址里没有任务 id');
      setPhase('error');
      return;
    }

    /** 清掉当前这一段实时文字，并钉在指定的一轮上；工具调用开始时前面的说明不算回答。 */
    const resetStream = (turn: number | null) => {
      streamTextRef.current = '';
      liveTurnRef.current = turn;
      useStreamStore.setState({
        turnId: turn === null ? null : 'turn-' + String(turn),
        itemId: null,
        text: '',
      });
    };

    /** 用户停在底部时跟着新事件走；往上翻的人不被打断。 */
    const stickToBottom = () => {
      const scroller = scrollRef.current;
      followBottomRef.current =
        scroller === null ||
        scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
    };

    // T039：任务会话里 task_report/task_handback 是终态事件；它们落盘后立刻重拉页头事实。
    const toolNameOfCall = new Map<string, string>();

    const onFrame = (frame: Frame) => {
      if (disposed) return;
      if (frame.type === 'event') {
        const event = frame.event;
        // seq=0 是设置变化的合成事件：不在任务日志里，和任务对话无关。
        if (event.seq === 0 || event.seq <= lastSeqRef.current) return;
        lastSeqRef.current = event.seq;
        const turn = typeof event.data?.turn === 'number' ? Number(event.data.turn) : null;
        // 页头的「在跑」跟着这一轮的起止走；停止后的收尾 turn/end 也会到这里。
        if (event.type === 'turn/start') {
          setTask((current) => (current ? { ...current, running: true } : current));
          resetStream(turn);
        }
        if (event.type === 'tool/call') {
          const callId = String(event.data?.call_id ?? event.data?.callId ?? '');
          if (callId) toolNameOfCall.set(callId, String(event.data?.name ?? ''));
          resetStream(liveTurnRef.current);
        }
        if (event.type === 'tool/result') {
          const callId = String(
            (event.data?.message as { toolCallId?: string; tool_call_id?: string } | undefined)
              ?.toolCallId ??
              (event.data?.message as { tool_call_id?: string } | undefined)?.tool_call_id ??
              '',
          );
          const name = callId ? toolNameOfCall.get(callId) : undefined;
          if (name === 'task_report' || name === 'task_handback') {
            // 报告落库后执行者会关掉会话（可能没有 turn/end），这里补一次刷新。
            setTimeout(() => void refreshHeader(), 200);
          }
        }
        if (event.type === 'turn/end') {
          setTask((current) => (current ? { ...current, running: false } : current));
          resetStream(null);
          // T039：一轮结束不能只信页面自己攒的事件：以任务库为准重拉一次页头事实。
          void refreshHeader();
        }
        stickToBottom();
        setEvents((current) => mergeLiveEvent(current, event));
        return;
      }
      if (frame.type !== 'agent') return;
      if (frame.event.type === 'tool-call-delta' || frame.event.type === 'retrying') {
        resetStream(liveTurnRef.current);
        return;
      }
      if (frame.event.type !== 'text-delta' || liveTurnRef.current === null) return;
      streamTextRef.current += frame.event.text;
      stickToBottom();
      useStreamStore.setState({
        turnId: 'turn-' + String(liveTurnRef.current),
        itemId: null,
        text: streamTextRef.current,
      });
    };

    const connect = () => {
      if (disposed) return;
      if (reconnectTimer !== null) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      let socket: WebSocket;
      socket = connectTaskSession(taskId, lastSeqRef.current, onFrame, () => {
        // 主动断开时先放行等待的人（发送前要让服务端放开会话写者）。
        const released = socketClosedRef.current;
        socketClosedRef.current = null;
        released?.();
        // 离开/换任务时旧 socket 的关闭不算断线；只有当前连接断了才重新跟。
        if (disposed || socketRef.current !== socket) return;
        socketRef.current = null;
        // T039：断线先按任务库刷一次（服务真没了就是失败，refreshHeader 自己吞掉）；
        // 断线期间任务被停/被打断/交报告，页面不靠重新打开也能自愈。
        void refreshHeader();
        reconnectAttempt += 1;
        const delay = Math.min(1000 * reconnectAttempt, 5000);
        reconnectTimer = setTimeout(connect, delay);
      });
      socketRef.current = socket;
      // T039：连上（含重启后的重连成功）先拉一次任务库事实，别等下一条日志事件。
      socket.addEventListener('open', () => {
        if (disposed || socketRef.current !== socket) return;
        void refreshHeader();
      });
      connectRef.current = connect;
    };

    void taskRead(taskId)
      .then((response) => {
        if (disposed || token !== requestToken.current) return;
        setTask(response.task);
        setLedger(response.ledger);
        setTaskTitle(response.task.title);
        setAttemptStats(response.stats ?? null);
        setFx(response.fx ?? null);
        setCurrency(response.currency ?? null);
        setEvents(response.events);
        setHasOlder(response.events.length > 0);
        lastSeqRef.current = response.lastSeq;
        // 第一页末尾可能正处在一轮中间：把流式文字钉到这一轮上。
        let running: number | null = null;
        for (const event of response.events) {
          const turn = typeof event.data?.turn === 'number' ? Number(event.data.turn) : null;
          if (turn === null) continue;
          if (event.type === 'turn/start') running = turn;
          if (event.type === 'turn/end' && running === turn) running = null;
        }
        liveTurnRef.current = running;
        setPhase('ready');
        connect();
        // T039：任务已交报告但会话可能还在收尾（终态报告后执行者立刻关会话，最后一句
        // 话/turn/end 可能晚几拍落盘）：分几次补拉，只把新 seq 并回事件流（幂等）。
        if (response.task.reportedAt != null && !response.task.running) {
          for (const delay of [600, 1600, 3200]) {
            setTimeout(() => {
              if (!disposed) void refreshHeader();
            }, delay);
          }
        }
      })
      .catch((reason: unknown) => {
        if (disposed || token !== requestToken.current) return;
        setError(reasonOf(reason));
        setPhase('error');
      });

    return () => {
      disposed = true;
      if (reconnectTimer !== null) clearTimeout(reconnectTimer);
      connectRef.current = null;
      const socket = socketRef.current;
      socketRef.current = null;
      socket?.close();
      const released = socketClosedRef.current;
      socketClosedRef.current = null;
      released?.();
      setTaskTitle(null);
      useStreamStore.setState({ turnId: null, itemId: null, text: '' });
    };
  }, [taskId]);

  // 首屏落到最新一条。
  useLayoutEffect(() => {
    if (phase !== 'ready' || bottomPinned.current) return;
    bottomPinned.current = true;
    const scroller = scrollRef.current;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [phase]);

  // 往上翻：新页插在前面，保持原来的阅读位置；装好后再消化一次加载期间收到的「到顶了」。
  useLayoutEffect(() => {
    const restore = restoreRef.current;
    if (!restore) return;
    restoreRef.current = null;
    const scroller = scrollRef.current;
    if (!scroller) return;
    scroller.scrollTop = scroller.scrollHeight - restore.height + restore.top;
    if (pendingOlderRef.current) {
      pendingOlderRef.current = false;
      if (scroller.scrollTop <= 64) void loadOlder();
    }
  }, [events]);

  // live 新事件：用户停在底部时跟着走。
  useLayoutEffect(() => {
    if (!followBottomRef.current) return;
    const scroller = scrollRef.current;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [events]);

  // 终态报告之后执行者会立刻关掉会话，日志可能没有 turn/end；头部带来的 reportedAt
  // 只收尾它之前开始的那一轮（waiting 报告之后的新一轮不能被标成已完成）。
  // T043：输入框下的任务级统计——只算当前 Attempt 的事件（task-read 给的就是这个 Attempt 的会话）。
  const views = useMemo(
    () =>
      withReportedCompletion(deriveTurnViews(visibleTaskEvents(events), turnLabels()), task?.reportedAt),
    [events, task],
  );

  // T052：下游子任务的任务包里有「上游结果」一段（assignment 里由内核写好）；任务对话里显示它。
  const upstreamResults = useMemo(() => {
    for (const event of events) {
      if (event.type !== 'user/message') continue;
      const message = event.data?.message as
        | { source?: { rpcId?: string }; content?: { type?: string; text?: string }[] }
        | undefined;
      if (!String(message?.source?.rpcId ?? '').startsWith(ASSIGNMENT_PREFIX)) continue;
      const text = (message?.content ?? []).map((block) => block?.text ?? '').join('');
      try {
        const parsed = JSON.parse(text) as {
          upstream_results?: {
            task_id: string;
            title: string;
            requirement_version: number;
            summary: string;
          }[];
        };
        return parsed.upstream_results ?? [];
      } catch {
        return [];
      }
    }
    return [];
  }, [events]);

  // 重启汇报送到以后横幅自己消失（任务仍是「被打断了」）；只在待核实时轻轮询页头事实。
  useEffect(() => {
    if (!task?.restartPending) return;
    const timer = window.setInterval(() => void refreshHeader(), 1000);
    return () => window.clearInterval(timer);
    // refreshHeader 只用 taskId；跟着 restartPending 重新挂。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task?.restartPending, taskId]);

  // T028.5：全局暂停从执行设置读（横幅 + 排队提示）；设置页改完回到本页也能读到。
  useEffect(() => {
    let live = true;
    void loadExecSettings()
      .then((view) => {
        if (live) setPaused(view.paused);
      })
      .catch(() => {
        // 读不到就不显示横幅；发送照旧由服务端闸门决定。
      });
    return () => {
      live = false;
    };
  }, [taskId, saying]);

  // T028.5：已经受理、日志里还没出现的输入（暂停时排队、没暂停时只是在送）。
  const queuedVisible = useMemo(() => {
    const seen = new Set<string>();
    for (const event of events) {
      const rpcId = rpcIdOf(event);
      if (rpcId?.startsWith(TASK_SAY_PREFIX)) seen.add(rpcId.slice(TASK_SAY_PREFIX.length));
    }
    return queued.filter((item) => !seen.has(item.inputId));
  }, [events, queued]);

  // T028.5：在跑时按 Esc 只停下这一轮（和 Kind 对话一样；任务不算停）。
  useEffect(() => {
    if (!task?.running) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      if (document.querySelector('[role="dialog"],[role="menu"]')) return;
      event.preventDefault();
      void stopTurn();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // stopTurn 只用 taskId，跟着 running 重新挂。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task?.running, taskId]);

  /**
   * T028.5：停下来以后说一句话之前，先放掉任务对话的连接。
   *
   * 任务在跑时页面这条 WS 订阅着这次 Attempt 的会话主机；Attempt 结束后服务端已经把
   * 它摘出在跑表，但连接还握着一个 Arc，会话写者不放开——这时「接着做」要在原会话
   * 开新 Attempt，会被写者挡住。先断开、发完再按 lastSeq 重新接上。
   */
  function releaseTaskSocket(): Promise<void> {
    const socket = socketRef.current;
    if (!socket) return Promise.resolve();
    return new Promise((resolve) => {
      let settled = false;
      const finish = () => {
        if (settled) return;
        settled = true;
        if (socketClosedRef.current === finish) socketClosedRef.current = null;
        resolve();
      };
      socketClosedRef.current = finish;
      // 等不到关闭也不卡住用户；服务端接着做时自己还会短暂等写者。
      setTimeout(finish, 500);
      socketRef.current = null;
      try {
        socket.close();
      } catch {
        finish();
      }
    });
  }

  /**
   * T028.5：新 Attempt 起来以后再接回实时帧。
   *
   * 发完立刻重连会撞上「Attempt 还没登记在跑表」：服务端只给一份只读快照、
   * 不出 live 帧，新 Attempt 的过程就永远不进来。这里短轮询 TaskRead 直到看见
   * running（或超时），顺带把轮询读到的日志补进列表，再接 WS。
   */
  async function reopenTaskStream(token: number) {
    for (let attempt = 0; attempt < 24; attempt += 1) {
      if (token !== requestToken.current) return;
      try {
        const response = await taskRead(taskId);
        if (token !== requestToken.current) return;
        setTask(response.task);
        setLedger(response.ledger);
        const fresh = response.events.filter((event) => event.seq > lastSeqRef.current);
        if (fresh.length > 0) {
          lastSeqRef.current = fresh[fresh.length - 1].seq;
          setEvents((current) => current.concat(fresh));
        }
        if (response.task.running) break;
      } catch {
        // 读不到就下一轮再试；超时后照样重新接连接。
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    if (token !== requestToken.current) return;
    connectRef.current?.();
  }

  /** T028.5：说一句（Composer 的发送；粘贴和附件照 Kind 对话一起送；成功才清草稿）。 */
  async function say(text: string, options: ComposerSendOptions) {
    if (!taskId || saying) return false;
    setSaying(true);
    try {
      // 停了/失败了的任务：先放开这条连接，服务端才能重新打开同一个会话。
      if (!task?.running) await releaseTaskSocket();
      const pastes = options.pastes;
      const attachments = options.attachments;
      const response = await taskSay(taskId, text, { pastes, attachments });
      // 说了以后（尤其新 Attempt）等它真的跑起来再接回实时帧。
      if (!socketRef.current) void reopenTaskStream(requestToken.current);
      setQueued((current) =>
        current.concat({ inputId: response.inputId, text, pastes, attachments }),
      );
      if (typeof response.newAttempt === 'number') {
        const attempt = response.newAttempt;
        setTask((current) => (current ? { ...current, attempts: attempt } : current));
      }
      // 暂停时消息先压着：横幅之外，刚说的那句以「排队中」的样子留在对话里。
      if (paused) {
        toast('先说上了，先排着', { description: '全局暂停中；开闸后送到 ThinAgent' });
      }
      return true;
    } catch (error) {
      toast('这句话没送到', { description: reasonOf(error) });
      return false;
    } finally {
      setSaying(false);
    }
  }

  /** T028.5：停下这一轮（不结束 Attempt、不出卡）；再说一句还是这次 Attempt 接着做。 */
  async function stopTurn() {
    try {
      const response = await taskInterrupt(taskId);
      if (!response.interrupted) {
        toast('这个任务没在跑', { description: response.message });
      }
      await refreshHeader();
    } catch (error) {
      toast('这一轮没停下来', { description: reasonOf(error) });
    }
  }

  /**
   * 刷新页头事实：running/stopped/interrupted/reportedAt 一律以任务库为准，页面不自己猜。
   * 断线重连可能连拉几次，只认最新一次的回包（旧回包不把新状态盖回去）。
   */
  async function refreshHeader() {
    const seq = ++headerRequest.current;
    try {
      const response = await taskRead(taskId);
      if (seq !== headerRequest.current) return;
      setLedger(response.ledger);
      // 重拉不只是页头：断线/关会话后晚落盘的尾巴（最后一句话、turn/end）也在这份里，
      // 按 seq 把缺口并回事件流（和 WS follow 的补齐幂等，重复的自然丢掉）。
      const fresh = response.events.filter((event) => event.seq > lastSeqRef.current);
      if (fresh.length > 0) {
        lastSeqRef.current = Math.max(lastSeqRef.current, response.lastSeq);
        setEvents((current) =>
          fresh.reduce((acc, event) => mergeLiveEvent(acc, event), current),
        );
      }
      setTask((current) =>
        current
          ? {
              ...current,
              running: response.task.running,
              stopped: response.task.stopped,
              interrupted: response.task.interrupted,
              restartPending: response.task.restartPending,
              reportedAt: response.task.reportedAt,
              attempts: response.task.attempts,
              report: response.task.report,
              review: response.task.review,
              accepted: response.task.accepted,
            }
          : response.task,
      );
      // 统计也以服务端为准（task-read 每次都带当前 Attempt 的 stats）。
      setAttemptStats(response.stats ?? null);
      setFx(response.fx ?? null);
      setCurrency(response.currency ?? null);
    } catch {
      // 服务没回来/读失败：保持现状（不清空、不覆盖成错误状态），等重连再拉。
    }
  }

  async function stopNow() {
    try {
      const response = await taskStop(taskId, 'now');
      if (!response.stopped) {
        toast('这个任务没在跑', { description: response.message });
        await refreshHeader();
        return;
      }
      setWinding(false);
      // 先按响应把页头定成已停止，再以任务库为准刷新；菜单立刻变灰。
      setTask((current) =>
        current ? { ...current, running: false, stopped: true } : current,
      );
      await refreshHeader();
    } catch (error) {
      toast('没停下来', { description: reasonOf(error) });
    }
  }

  async function stopAfterReport() {
    try {
      const response = await taskStop(taskId, 'after_report');
      if (!response.stopped) {
        toast('这个任务没在跑', { description: response.message });
        await refreshHeader();
        return;
      }
      setWinding(true);
      toast('收尾停已插进去', {
        description: '做完手上这一步、交一份报告就结束；之前别的工具一律拒绝',
      });
      await refreshHeader();
    } catch (error) {
      toast('收尾停没插进去', { description: reasonOf(error) });
    }
  }

  async function loadOlder() {
    // 这一页还在读：记下这次到顶，装好（或失败）后由恢复滚动的那一步再判。
    if (loadingOlderRef.current) {
      pendingOlderRef.current = true;
      return;
    }
    if (!hasOlder || events.length === 0) return;
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
      pendingOlderRef.current = false;
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
    if (!scroller) return;
    followBottomRef.current =
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
    if (phase !== 'ready' || scroller.scrollTop > 64) return;
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
            <TaskState
              running={task.running}
              winding={winding}
              stopped={task.stopped}
              interrupted={task.interrupted}
              report={task.report ?? null}
              review={task.review}
              accepted={task.accepted}
            />
            <TaskMenu
              running={task.running}
              winding={winding}
              onStopNow={() => void stopNow()}
              onStopAfterReport={() => void stopAfterReport()}
            />
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-2 text-ink-3">
            {task.writeScope ? (
              <span
                className="flex items-center gap-1 rounded-badge bg-wait-soft px-1.5 text-wait"
                data-kind="task-write-scope"
              >
                <FilePen className="size-3.5" />
                写任务 · 写入范围 <span className="process-path">{task.writeScope}</span>
                {task.bash ? ' · 开了 bash（不受写入范围约束）' : ''}
              </span>
            ) : (
              <span>调查任务 · 只读</span>
            )}
            <span className="flex items-center gap-1">
              <FolderOpen className="size-3.5" />
              <span className="process-path">{displayPath(task.cwd)}</span>
            </span>
            <span>ThinAgent · {task.model}</span>
            {task.attempts > 1 ? (
              <span data-kind="task-attempt">第 {task.attempts} 次 Attempt</span>
            ) : null}
          </div>
          <Brief task={task} />
          {ledger ? <Ledger ledger={ledger} task={task} /> : null}
        </div>
      </div>

      {paused ? (
        <div
          className="flex shrink-0 items-center gap-2 border-b border-wait/30 bg-wait-soft px-6 py-2 text-2 text-wait"
          data-kind="paused"
        >
          <PauseCircle className="size-3.5" />
          全局暂停中：你的消息会先排着；开闸后送到
        </div>
      ) : null}

      {task.restartPending ? (
        <div
          className="flex shrink-0 items-center gap-2 border-b border-wait/30 bg-wait-soft px-6 py-2 text-2 text-wait"
          data-kind="await-kind"
        >
          <PlugZap className="size-3.5" />
          被打断了 · 待 Kind 核实，尚未接续
        </div>
      ) : null}

      {upstreamResults.length > 0 ? (
        <div className="shrink-0 border-b border-line bg-hover px-6 py-2" data-kind="upstream-results">
          <div className="mx-auto w-full max-w-[680px]">
            <div className="text-1 text-ink-3">上游结果</div>
            <ul className="mt-1 space-y-1 text-2 leading-5 text-ink-2">
              {upstreamResults.map((item) => (
                <li key={item.task_id}>
                  <span className="text-ink">{item.title}</span>
                  <span className="text-ink-3">（要求第 {item.requirement_version} 版）</span>{' '}
                  {item.summary}
                </li>
              ))}
            </ul>
          </div>
        </div>
      ) : null}

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
          {queuedVisible.map((item) => {
            const hasCards = item.pastes.length > 0 || item.attachments.length > 0;
            const user: TurnUserView = {
              text: '',
              elements: [],
              pastes: item.pastes,
              files: item.attachments,
              canvas: null,
              resultCard: null,
            };
            return (
              <div
                key={item.inputId}
                className="px-6 pb-2"
                data-kind="task-queued"
                data-state={paused ? 'paused' : 'sending'}
              >
                <div className="mt-3 flex flex-col items-end gap-1">
                  {hasCards ? <UserCards user={user} /> : null}
                  {item.text ? (
                    <div
                      className={cn(
                        'max-w-[82%] rounded-2xl rounded-br-md border border-line-2 bg-raised/60 px-3.5 py-2 text-4 leading-6 text-ink-2',
                        paused && 'border-dashed',
                      )}
                    >
                      {item.text}
                    </div>
                  ) : null}
                  <div className="text-1 text-ink-3">
                    {paused ? '排队中：全局暂停，开闸后送到' : '发送中…'}
                  </div>
                </div>
              </div>
            );
          })}
          {loadingOlder ? (
            <div className="px-6 py-2 text-center text-1 text-ink-3" data-kind="task-loading-older">
              正在读更早的记录…
            </div>
          ) : null}
        </div>
      </div>

      <div className="shrink-0 border-t border-line px-6 pb-2 pt-3">
        <div className="mx-auto w-full max-w-[680px]">
          <Composer
            ref={composerRef}
            offline={false}
            draftKey={'task-' + taskId}
            placeholder={
              task.running
                ? '插一句给 ThinAgent（这一轮里就会看到）…'
                : '和 ThinAgent 说，它就接着做…'
            }
            onSend={say}
            onCanSendChange={() => undefined}
          >
            {task.running ? (
              <button
                type="button"
                onClick={() => void stopTurn()}
                title="停下这一轮（Esc）：任务不算停，接着说就接着做"
                aria-label="停下这一轮"
                data-kind="task-stop-turn"
                className="grid size-7 place-items-center rounded-full bg-ink text-content transition-opacity hover:opacity-90"
              >
                <Square className="size-3 fill-current" />
              </button>
            ) : (
              <button
                type="button"
                onClick={() => composerRef.current?.submit()}
                title="发送"
                aria-label="发送"
                data-kind="task-send"
                className="grid size-7 place-items-center rounded-full bg-ink text-content transition-opacity hover:opacity-90"
              >
                <SendHorizontal className="size-3.5" />
              </button>
            )}
          </Composer>
          {attemptStats === null ? null : (
            <TaskStatsDock
              stats={attemptStats}
              fx={fx}
              currency={currency}
              attempts={task.attempts}
            />
          )}
          <p className="mt-1 text-1 leading-4 text-ink-3" data-kind="task-footer">
            你在这里说的直接给 ThinAgent，同时抄送 Kind（不叫醒它）。要改任务要求或 DoD，回 Kind
            对话说。
          </p>
        </div>
      </div>
    </div>
  );
}
