// T007.3/4：页面与 aios-dev serve 的线上客户端（只加页面要用的方法/帧）。
// 协议事实：POST /api {method,...} → Response；WS /ws 先 ready 再事件/agent 帧。

export interface ServiceEvent {
  seq: number;
  /** 日志事件发生时间（unix 毫秒；工作过程用）。 */
  time: number;
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

export class ServiceHttpError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

export interface ServiceTarget {
  origin: string;
  token: string;
}

/** 页面访问同源服务；认证靠首页换到的签名 cookie（D49/T006.F2）。 */
export function serviceTarget(): ServiceTarget {
  return { origin: window.location.origin, token: '' };
}

async function post<T>(body: Record<string, unknown>): Promise<T> {
  const target = serviceTarget();
  const response = await fetch(`${target.origin}/api`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw new ServiceHttpError(response.status, `服务拒绝：${response.status}`);
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

export interface ServiceAnswer {
  questionId: string;
  selected: string[];
  custom?: string;
}

/** 整批回答（T007.6）：每题 {questionId, selected[], custom?}，一次调用覆盖一批。 */
export async function answerQuestions(answers: ServiceAnswer[]): Promise<void> {
  await post({ method: 'answer', answers });
}

/** 整批不答（T007.6）：撤掉当前全部待答问题，模型收到用户没答后照常收尾。 */
export async function dismissQuestions(): Promise<void> {
  await post({ method: 'dismiss-questions' });
}

/** 手动压缩（T007.8）：把之前的对话换成一份摘要，原文留在日志里。 */
export async function compactThread(commandId: string): Promise<void> {
  await post({ method: 'compact', commandId });
}

export async function recheckRoot(): Promise<ServiceRootState> {
  const response = await post<{ root: ServiceRootState }>({ method: 'recheck' });
  return response.root;
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


/* ───────────────────────── T008 设置 › 模型与连接（Keyring） ───────────────────────── */

/** 与 service 协议 v3 对应的 Keyring 类型（响应里没有 key 正文）。 */
export interface ServiceKeyFileInfo {
  path: string;
  since: number;
  problem?: string;
}

export interface ServiceConnectionInfo {
  id: string;
  name: string;
  model: string;
  endpoint: string;
  saved_at: number;
  key_saved: boolean;
  check: { kind: 'never' } | { kind: 'ok'; at: number } | { kind: 'fail'; at: number; reason: string };
}

export interface ServiceIssueRecord {
  id: number;
  at: number;
  connection: string;
  purpose: string;
  run?: string;
}

export interface ServiceKeyringView {
  connections: ServiceConnectionInfo[];
  key_file: ServiceKeyFileInfo;
  issued_today: number;
  issues: ServiceIssueRecord[];
  first_use: boolean;
}

export interface SaveConnectionInput {
  id: string;
  name: string;
  model: string;
  endpoint: string;
  apiKey: string;
}

/** 设置页读取（不含 key 正文）。 */
export async function fetchModelSettings(): Promise<ServiceKeyringView> {
  const response = await post<{ settings: ServiceKeyringView }>({ method: 'model-settings' });
  return response.settings;
}

/** 存/换连接；key 只在这一次请求体里。 */
export async function saveConnection(input: SaveConnectionInput): Promise<ServiceConnectionInfo> {
  const response = await post<{ connection: ServiceConnectionInfo }>({
    method: 'connection-save',
    ...input,
  });
  return response.connection;
}

export async function deleteConnection(id: string): Promise<boolean> {
  const response = await post<{ deleted: boolean }>({ method: 'connection-delete', id });
  return response.deleted;
}

/** 验证连接；结果落在注册器（响应不带 key）。 */
export async function verifyConnection(id: string): Promise<ServiceConnectionInfo> {
  const response = await post<{ connection: ServiceConnectionInfo }>({
    method: 'connection-verify',
    id,
  });
  return response.connection;
}

export async function relocateKeyring(to: string): Promise<void> {
  await post({ method: 'keyring-relocate', to });
}

export async function reopenKeyring(at: string): Promise<void> {
  await post({ method: 'keyring-reopen', at });
}

export async function restartKeyring(): Promise<ServiceKeyringView> {
  const response = await post<{ settings: ServiceKeyringView }>({ method: 'keyring-restart' });
  return response.settings;
}

export async function fetchIssues(): Promise<{ issuedToday: number; issues: ServiceIssueRecord[] }> {
  const response = await post<{ issuedToday: number; issues: ServiceIssueRecord[] }>({
    method: 'issues',
  });
  return response;
}

export async function fetchFirstUse(): Promise<{ firstUse: boolean; keyFile: ServiceKeyFileInfo }> {
  const response = await post<{ firstUse: boolean; keyFile: ServiceKeyFileInfo }>({
    method: 'first-use',
  });
  return response;
}

/** 选模型：下一轮生效，正在跑的轮不换（D26）。 */
export async function selectSessionModel(model: string): Promise<string> {
  const response = await post<{ model: string }>({ method: 'session-model-select', model });
  return response.model;
}

/** 连 WS：ready → 补齐/直播的日志事件（event）与不落日志的 agent 帧（流式文本等）。 */
export function connectSession(
  lastSeq: number,
  onFrame: (frame: Frame) => void,
  onClose: () => void,
): WebSocket | null {
  const target = serviceTarget();
  const socket = new WebSocket(
    `${target.origin.replace('http', 'ws')}/ws?lastSeq=${lastSeq}`,
  );
  socket.onmessage = (message) => {
    try {
      onFrame(JSON.parse(String(message.data)) as Frame);
    } catch {
      // 坏帧不打断页面；真正缺事件由重连补齐（T007.9）。
    }
  };
  socket.onclose = onClose;
  socket.onerror = onClose;
  return socket;
}

/** T008.3 增补：只改 endpoint/model，保留已存 key。 */
export async function updateConnectionInfo(
  id: string,
  endpoint: string,
  model: string,
): Promise<ServiceConnectionInfo> {
  const response = await post<{ connection: ServiceConnectionInfo }>({
    method: 'connection-update',
    id,
    endpoint,
    model,
  });
  return response.connection;
}

/** T008.3 增补：撤销 key（连接保留）。 */
export async function revokeConnectionKey(id: string): Promise<ServiceConnectionInfo> {
  const response = await post<{ connection: ServiceConnectionInfo }>({
    method: 'connection-revoke',
    id,
  });
  return response.connection;
}
