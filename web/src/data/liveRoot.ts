/**
 * 一根共用的根会话 WS，页面只按事件重读，不轮询。
 *
 * 工作页/等你/Kind 任务卡/日历都挂在这一根上；收到什么帧由订阅者自己过滤。
 * 断线按 reconnect.ts 的退避重连（300ms 起翻倍、封顶 5s）；收到第一帧后退避归零。
 */
import type { Frame } from '@/data/generated/Frame';
import { reconnectDelayMs } from '@/data/reconnect';
import { connectSession } from '@/data/service';

type FrameHandler = (frame: Frame) => void;

const handlers = new Set<FrameHandler>();
let socket: WebSocket | null = null;
let reconnectTimer: number | null = null;
let attempt = 0;

function open(): void {
  if (socket !== null || handlers.size === 0) return;
  socket = connectSession(
    Number.MAX_SAFE_INTEGER,
    (frame) => {
      attempt = 0;
      for (const handler of handlers) handler(frame);
    },
    () => {
      socket = null;
      if (handlers.size === 0) return;
      attempt += 1;
      const delay = reconnectDelayMs(attempt);
      if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = null;
        open();
      }, delay);
    },
  );
}

/** 订阅根会话帧；返回取消订阅。第一个订阅者打开连接，最后一个离开才关。 */
export function subscribeRootFrames(handler: FrameHandler): () => void {
  handlers.add(handler);
  open();
  return () => {
    handlers.delete(handler);
    if (handlers.size > 0) return;
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    socket?.close();
    socket = null;
    attempt = 0;
  };
}
