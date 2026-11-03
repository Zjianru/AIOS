// T007.3/4：页面与 aios-dev serve 的线上客户端（只加页面要用的方法/帧）。
// 协议事实：POST /api {method,...} → Response；WS /ws 先 ready 再事件/agent 帧。

export interface ServiceEvent {
  seq: number;
  type: string;
  data: Record<string, unknown>;
  cause?: 'credential' | 'model';
}

export type ServiceRootState =
  | { state: 'ok' }
  | {
      state: 'unavailable';
      kind: 'lost' | 'corrupt' | 'busy' | 'other';
      path?: string;
      line?: number;
      message?: string;
    };

export interface ServiceContext {
  used: number;
  window: number;
}

export interface ReadyFrame {
  type: 'ready';
  sessionId?: string;
  protocolVersion: number;
  root: ServiceRootState;
  context: ServiceContext;
}

export interface EventFrame {
  type: 'event';
  event: ServiceEvent;
}

export interface AgentWireEvent {
  type: 'text-delta' | 'usage' | 'retrying';
  text?: string;
  attempt?: number;
  maxAttempts?: number;
  delayMs?: number;
  code?: string;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
}

export interface AgentFrame {
  type: 'agent';
  event: AgentWireEvent;
}

export type Frame = ReadyFrame | EventFrame | AgentFrame;

export interface ServiceTarget {
  origin: string;
  token: string;
}

/** 从地址栏取启动令牌（T006 的信任栅栏：令牌在打印的地址里）。 */
export function serviceTarget(): ServiceTarget | null {
  const token = new URLSearchParams(window.location.search).get('token');
  if (!token) return null;
  return { origin: window.location.origin, token };
}

async function post<T>(body: Record<string, unknown>): Promise<T> {
  const target = serviceTarget();
  if (!target) throw new Error('地址里没有启动令牌');
  const response = await fetch(`${target.origin}/api?token=${encodeURIComponent(target.token)}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw new Error(`服务拒绝：${response.status}`);
  }
  return (await response.json()) as T;
}

/** 发消息（requestId 是幂等键：重发同一个 id 不会让 Kind 收到两次）。 */
export async function sendMessage(content: string, requestId: string): Promise<void> {
  await post({ method: 'send', requestId, content });
}

/** 取消当前轮（T007.5 停止按钮 / Esc；服务端取消正在跑的一轮并保留已做的）。 */
export async function cancelTurn(): Promise<void> {
  await post({ method: 'cancel' });
}

export interface EventsPage {
  events: ServiceEvent[];
  hasMore: boolean;
}

/** events：往回翻一页日志（T006.16c 的轮对齐预算；打开/往上翻用）。 */
export async function fetchEventsPage(options: {
  beforeSeq?: number;
  maxMessages?: number;
  minMessages?: number;
  minTurns?: number;
}): Promise<EventsPage> {
  const response = await post<{ events: ServiceEvent[]; hasMore: boolean }>({
    method: 'events',
    ...options,
  });
  return { events: response.events, hasMore: response.hasMore };
}

/** 连 WS：ready → 补齐/直播的日志事件（event）与不落日志的 agent 帧（流式文本等）。 */
export function connectSession(
  lastSeq: number,
  onFrame: (frame: Frame) => void,
  onClose: () => void,
): WebSocket | null {
  const target = serviceTarget();
  if (!target) return null;
  const socket = new WebSocket(
    `${target.origin.replace('http', 'ws')}/ws?token=${encodeURIComponent(target.token)}&lastSeq=${lastSeq}`,
  );
  socket.onmessage = (message) => {
    try {
      onFrame(JSON.parse(String(message.data)) as Frame);
    } catch {
      // 坏帧不打断页面；真正缺事件由重连补齐（T007.9）。
    }
  };
  socket.onclose = onClose;
  return socket;
}
