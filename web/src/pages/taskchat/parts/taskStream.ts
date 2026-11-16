//! 任务对话的 WS 连接辅助：发话前先放掉连接，让服务端放开会话写者。

import type { MutableRefObject } from 'react';

/**
 * 停下来以后说一句话之前，先放掉任务对话的连接。
 *
 * 任务在跑时页面这条 WS 订阅着这次 Attempt 的会话主机；Attempt 结束后服务端已经把它
 * 摘出在跑表，但连接还握着一个 Arc，会话写者不放开——这时「接着做」要在原会话开新
 * Attempt，会被写者挡住。先断开、发完再按 lastSeq 重新接上。
 */
export function releaseTaskSocketRefs(
  socketRef: MutableRefObject<WebSocket | null>,
  socketClosedRef: MutableRefObject<(() => void) | null>,
): Promise<void> {
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
