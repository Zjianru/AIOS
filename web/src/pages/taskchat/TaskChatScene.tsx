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

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ChevronDown, FilePen, FolderOpen, PauseCircle, PlugZap, SendHorizontal, Square } from 'lucide-react';
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { useTaskChromeStore } from '@/app/taskChrome';
import { actionAct, actionDescribe, connectTaskSession, displayPath, taskDelivery, taskInterrupt, taskRead, taskSay, taskStop } from '@/data/service';
import type { Frame, SendAttachmentRef, ServiceEvent } from '@/data/service';
import type { DeliveryRecord } from '@/data/generated/DeliveryRecord';
import type { TaskReadHeader } from '@/data/generated/TaskReadHeader';
import type { TaskReadLedger } from '@/data/generated/TaskReadLedger';
import type { SessionStats } from '@/data/generated/SessionStats';
import type { FxInfo } from '@/data/generated/FxInfo';
import { useStreamStore } from '@/pages/kind/kindStore';
import { DeliveryBlock } from './DeliveryBlock';
import { TurnRow, UserCards } from '@/pages/kind/TurnRow';
import type { TurnUserView } from '@/pages/kind/turnViews';
import { Composer } from '@/pages/kind/composer/Composer';
import type { ComposerHandle, ComposerSendOptions } from '@/pages/kind/composer/Composer';
import { mergeLiveEvent } from './taskLive';
import { TaskStatsDock } from './TaskStatsDock';
import { cn } from '@/lib/utils';

import { BackToKind, TaskMenu } from './parts/menu';
import { Brief } from './parts/brief';
import { Ledger } from './parts/ledger';
import { ProgramSection, ProgramState } from './parts/program';
import { ReviewSection } from './parts/review';
import { TaskState } from './parts/state';
import { TaskProjectPicker } from './parts/project';
import { NO_CANVAS, NO_COUNTS, reasonOf } from './parts/helpers';
import { subscribeRootFrames } from '@/data/liveRoot';
import { reconnectDelayMs } from '@/data/reconnect';
import { useTaskChatSideData } from './parts/useTaskChatSideData';
import { releaseTaskSocketRefs } from './parts/taskStream';


export function TaskChatScene() {
  const taskId = useRouteStore((state) => state.route.id);
  const setTaskTitle = useTaskChromeStore((state) => state.setTaskTitle);
  const [phase, setPhase] = useState<'loading' | 'ready' | 'error'>('loading');
  const [task, setTask] = useState<TaskReadHeader | null>(null);
  const [ledger, setLedger] = useState<TaskReadLedger | null>(null);
  const [delivery, setDelivery] = useState<DeliveryRecord | null>(null);
  const [deliveryNonce, setDeliveryNonce] = useState(0);
  const [upstreamOpen, setUpstreamOpen] = useState(false);
  // T051：统计由服务端 task-read 给（和 Kind status 同一个 session_stats），页面只显示。
  const [attemptStats, setAttemptStats] = useState<SessionStats | null>(null);
  const [fx, setFx] = useState<FxInfo | null>(null);
  const [currency, setCurrency] = useState<string | null>(null);
  const [winding, setWinding] = useState(false);
  const [events, setEvents] = useState<ServiceEvent[]>([]);
  const [error, setError] = useState('');
  const [hasOlder, setHasOlder] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  /** T028.5：说的是不是正在送；送的时候按钮不重复触发。 */
  const [saying, setSaying] = useState(false);
  /** T028.5：已经受理、还没出现在会话日志里的输入（排队或正在送）。 */
  const [queued, setQueued] = useState<
    { inputId: string; text: string; pastes: string[]; attachments: SendAttachmentRef[] }[]
  >([]);
  const composerRef = useRef<ComposerHandle | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  /** 钉底走 scrollTo，不直接写 scrollTop：流式追加时内容高度布局后才定。 */
  const pinToLatest = useCallback(() => {
    const scroller = scrollRef.current;
    scroller?.scrollTo({ top: scroller.scrollHeight, behavior: 'auto' });
  }, []);
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
      if (frame.type === 'task-changed') {
        setDeliveryNonce((value) => value + 1);
        void refreshHeader();
        return;
      }
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
        reconnectTimer = setTimeout(connect, reconnectDelayMs(reconnectAttempt));
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
        // 任务已交报告但会话可能还在收尾：等这个任务的 task-changed，不再按
        // [600,1600,3200] 固定补拉；保留一次 10 秒兜底。
        if (response.task.reportedAt != null && !response.task.running) {
          let settled = false;
          let timer: number | null = null;
          const unsubscribe = subscribeRootFrames((frame) => {
            if (frame.type !== 'task-changed' || frame.taskId !== taskId) return;
            settled = true;
            unsubscribe();
            if (timer !== null) window.clearTimeout(timer);
            if (!disposed) void refreshHeader();
          });
          timer = window.setTimeout(() => {
            if (settled) return;
            settled = true;
            unsubscribe();
            if (!disposed) void refreshHeader();
          }, 10_000);
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
    pinToLatest();
  }, [phase, pinToLatest]);

  // 往上翻：新页插在前面，保持原来的阅读位置；装好后再消化一次加载期间收到的「到顶了」。
  useLayoutEffect(() => {
    const restore = restoreRef.current;
    if (!restore) return;
    restoreRef.current = null;
    const scroller = scrollRef.current;
    if (!scroller) return;
    scroller.scrollTo({
      top: scroller.scrollHeight - restore.height + restore.top,
      behavior: 'auto',
    });
    if (pendingOlderRef.current) {
      pendingOlderRef.current = false;
      if (scroller.scrollTop <= 64) void loadOlder();
    }
  }, [events]);

  // live 新事件：用户停在底部时跟着走。
  useLayoutEffect(() => {
    if (!followBottomRef.current) return;
    pinToLatest();
  }, [events, pinToLatest]);

  // 流式长高时再钉一次：内容高度在布局后才定，只看 events 会落在旧高度上。
  useEffect(() => {
    const content = contentRef.current;
    if (phase !== 'ready' || !content) return;
    const observer = new ResizeObserver(() => {
      if (followBottomRef.current) pinToLatest();
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, [phase, pinToLatest]);

  const { paused, views, upstreamResults, queuedVisible } = useTaskChatSideData({
    events,
    task,
    taskId,
    saying,
    queued,
    refreshHeader,
    stopTurn,
  });

  /**
   * T028.5：新 Attempt 起来以后再接回实时帧。
   *
   * 发完立刻重连会撞上「Attempt 还没登记在跑表」：服务端只给一份只读快照、
   * 不出 live 帧，新 Attempt 的过程就永远不进来。这里短轮询 TaskRead 直到看见
   * running（或超时），顺带把轮询读到的日志补进列表，再接 WS。
   */
  async function reopenTaskStream(token: number) {
    // 新 Attempt 登记会先落库并发 task-changed；等它再重连，不短轮询。
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
      if (!response.task.running) {
        await new Promise<void>((resolve) => {
          let timer: number | null = null;
          const finish = () => {
            unsubscribe();
            if (timer !== null) window.clearTimeout(timer);
            resolve();
          };
          const unsubscribe = subscribeRootFrames((frame) => {
            if (frame.type === 'task-changed' && frame.taskId === taskId) finish();
          });
          timer = window.setTimeout(finish, 10_000);
        });
      }
    } catch {
      // 读不到也让后面重连接一次。
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
      if (!task?.running) await releaseTaskSocketRefs(socketRef, socketClosedRef);
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
              backgroundRunning: response.task.backgroundRunning,
              stopped: response.task.stopped,
              interrupted: response.task.interrupted,
              restartPending: response.task.restartPending,
              reportedAt: response.task.reportedAt,
              attempts: response.task.attempts,
              report: response.task.report,
              review: response.task.review,
              accepted: response.task.accepted,
              cancelledAt: response.task.cancelledAt,
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

  async function cancelTask() {
    try {
      const details = { task_ids: [taskId] };
      const described = (await actionDescribe('cancel_tasks', details)) as unknown as {
        facts: { name: string; value: string }[];
      };
      const lines = described.facts.map((fact) => fact.name + '：' + fact.value).join('\n');
      if (!window.confirm('要取消这些任务吗？\n\n' + lines)) return;
      const outcome = (await actionAct('cancel_tasks', details)) as unknown as {
        summary: string;
      };
      toast('取消完成', { description: outcome.summary });
      await refreshHeader();
    } catch (error) {
      toast('没取消', { description: reasonOf(error) });
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

  const deliveryKey = task
    ? [
        task.reportedAt ?? 0,
        task.accepted ? 1 : 0,
        task.gate?.conclusion ?? '',
        task.gate?.runs?.[0]?.at ?? 0,
        task.guardingGate?.conclusion ?? '',
        task.guardingGate?.commandsVersion ?? 0,
        task.guardingGate?.at ?? 0,
      ].join('|')
    : '';
  useEffect(() => {
    if (!task || task.gate) {
      setDelivery(null);
      return;
    }
    let live = true;
    void taskDelivery(taskId)
      .then((response) => {
        if (live) setDelivery(response.delivery ?? null);
      })
      .catch(() => {
        if (live) setDelivery(null);
      });
    return () => {
      live = false;
    };
    // reportedAt/accepted/门判定变时重读；不挂轮询。
  }, [taskId, deliveryKey, deliveryNonce]);

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

  const program = task.gate && task.gate.commands.length > 0 ? task.gate : null;
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
            {program ? (
              <ProgramState running={task.program_running} gate={program} />
            ) : (
              <TaskState
                running={task.running}
                winding={winding}
                stopped={task.stopped}
                interrupted={task.interrupted}
                report={task.report ?? null}
                review={task.review}
                accepted={task.accepted}
              />
            )}
            <button
              type="button"
              data-kind="task-diagnose"
              onClick={() => go('monitor:task:' + task.id)}
              className="flex shrink-0 items-center gap-0.5 rounded-btn px-1.5 py-1 text-2 text-ink-3 transition-colors hover:bg-hover hover:text-ink"
            >
              {'诊断 ›'}
            </button>
            <TaskMenu
              running={task.running || task.backgroundRunning}
              winding={winding}
              program={program !== null}
              cancelled={Boolean(task.cancelledAt)}
              onStopNow={() => void stopNow()}
              onStopAfterReport={() => void stopAfterReport()}
              onCancel={() => void cancelTask()}
            />
          </div>
          <div className="mt-1">
            <TaskProjectPicker
              task={task}
              onChanged={(projectId) =>
                setTask((current) => (current ? { ...current, projectId: projectId ?? undefined } : current))
              }
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
              {task.cwdSource === 'project' ? (
                <span data-kind="task-cwd-source">项目默认目录</span>
              ) : null}
            </span>
            <span>{program ? '程序门 · 自动检查' : 'ThinAgent · ' + task.model}</span>
            {!program && task.attempts > 1 ? (
              <span data-kind="task-attempt">第 {task.attempts} 次 Attempt</span>
            ) : null}
          </div>
          {program ? (
            <ProgramSection
              gate={program}
              running={task.program_running}
              index={task.program_index ?? null}
            />
          ) : (
            <>
              <Brief task={task} />
              {ledger ? <Ledger ledger={ledger} task={task} /> : null}
              {task.gate && !program ? <ReviewSection gate={task.gate} /> : null}
              <DeliveryBlock delivery={delivery} />
            </>
          )}
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
        <div
          className="flex max-h-[35vh] shrink-0 flex-col border-b border-line bg-hover px-6 py-2"
          data-kind="upstream-results"
        >
          <div className="mx-auto flex min-h-0 w-full max-w-[680px] flex-col">
            <button
              type="button"
              className="flex w-full items-center gap-1 text-left text-1 text-ink-3 hover:text-ink"
              data-kind="upstream-toggle"
              onClick={() => setUpstreamOpen((open) => !open)}
            >
              <ChevronDown
                className={cn('size-3.5 transition-transform', upstreamOpen ? '' : '-rotate-90')}
              />
              上游结果 · {upstreamResults.length} 项
              <span className="ml-auto">{upstreamOpen ? '收起' : '展开'}</span>
            </button>
            {upstreamOpen ? (
              <div
                className="mt-1 min-h-0 overflow-y-auto overscroll-contain pr-1"
                data-kind="upstream-scroll"
              >
                <ul className="space-y-1 text-2 leading-5 text-ink-2">
                  {upstreamResults.map((item) => (
                    <li key={item.task_id}>
                      <div className="line-clamp-2 text-ink">
                        {item.title}
                        <span className="text-ink-3">（要求第 {item.requirement_version} 版）</span>
                      </div>
                      <div className="whitespace-pre-wrap break-words">{item.summary}</div>
                    </li>
                  ))}
                </ul>
              </div>
            ) : (
              <ul className="mt-0.5 space-y-0.5">
                {upstreamResults.map((item) => (
                  <li key={item.task_id} className="line-clamp-2 text-2 text-ink-2">
                    <span className="text-ink">{item.title}</span>
                    <span className="text-ink-3">（要求第 {item.requirement_version} 版）</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      ) : null}

      {program ? null : (
      <div
        className="min-h-[40vh] flex-1 overflow-y-auto"
        data-kind="task-scroll"
        ref={scrollRef}
        onScroll={onScroll}
      >
        <div ref={contentRef} className="mx-auto w-full max-w-[680px] pb-6" data-conversation="task">
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
              rpcId: null,
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
      )}

      {task.cancelledAt ? (
        <div className="shrink-0 border-t border-line px-6 py-3">
          <div className="mx-auto w-full max-w-[680px]">
            <p className="text-2 text-ink-3" data-kind="task-cancelled-input">
              这个任务已经取消了；要再做，让 Kind 新派一个。
            </p>
          </div>
        </div>
      ) : program ? (
        <div className="shrink-0 border-t border-line px-6 py-3">
          <div className="mx-auto w-full max-w-[680px]">
            <p className="text-2 text-ink-3" data-kind="task-footer">
              程序门没有对话：命令是程序跑的，不经过模型。要改命令、要重跑，回 Kind 对话说；改命令会出一张新的确认卡。
            </p>
          </div>
        </div>
      ) : (
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
            {task.running || task.backgroundRunning ? (
              <button
                type="button"
                onClick={() => void stopTurn()}
                title={
                  task.backgroundRunning
                    ? '停下报告后还在跑的后台命令'
                    : '停下这一轮（Esc）：任务不算停，接着说就接着做'
                }
                aria-label={task.backgroundRunning ? '停下后台命令' : '停下这一轮'}
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
      )}
    </div>
  );
}
