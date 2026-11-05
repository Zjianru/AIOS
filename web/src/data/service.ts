// T007.3/4：页面与 aios-dev serve 的线上客户端（只加页面要用的方法/帧）。
// 协议事实：POST /api {method,...} → Response；WS /ws 先 ready 再事件/agent 帧。

import { apiErrorReason } from './errors';

export interface ServiceEvent {
  seq: number;
  /** 日志事件发生时间（unix 毫秒；工作过程用）。 */
  time: number;
  type: string;
  data: Record<string, unknown>;
  cause?: 'credential' | 'model' | 'keyring';
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
  /** T009.5：宿主位置；页面只把它用于缩写显示的路径。 */
  host?: { home?: string };
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

// T009.5：ready 帧给的宿主 home；页面把显示的路径缩写成 ~/…，不改服务端事实。
let hostHomeValue: string | null = null;

export function setHostHome(home: string | null | undefined): void {
  hostHomeValue = home && home.length > 0 ? home.replace(/\/$/, '') : null;
}

/** 把宿主机上的绝对路径缩写成 ~/…（只用于显示；拼给服务端的路径不变）。 */
export function displayPath(raw: string | null | undefined): string {
  if (!raw) return '';
  const home = hostHomeValue;
  if (!home) return raw;
  if (raw === home) return '~';
  if (raw.startsWith(home + '/')) return '~' + raw.slice(home.length);
  return raw;
}

export class ServiceHttpError extends Error {
  status: number;
  code?: string;
  constructor(status: number, message: string, code?: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

// T008 r4 #1：页面任何调用拿到 401 都进令牌说明页；401 的判断只在这一处。
let onUnauthorized: (() => void) | null = null;

export function setUnauthorizedHandler(handler: () => void): void {
  onUnauthorized = handler;
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
  if (response.status === 401) {
    onUnauthorized?.();
    throw new ServiceHttpError(response.status, '登录已失效：请重新打开终端里打印的地址');
  }
  if (!response.ok) {
    // T009.2：/api 错误体是 {code,message,details?}；页面只显示对照表的中文原因，不显示码/状态。
    let code: string | undefined;
    try {
      const body = (await response.json()) as { code?: unknown };
      if (typeof body.code === 'string') code = body.code;
    } catch {
      // 非 JSON 错误体：走通用原因。
    }
    throw new ServiceHttpError(response.status, apiErrorReason(code), code);
  }
  return (await response.json()) as T;
}

/** T011.6：页面没接住的异常、事件流重连，报给服务记监控（限长；失败静默，不阻塞页面）。 */
export async function reportMonitor(
  kind: 'page-error' | 'page-reconnect',
  message: string,
): Promise<void> {
  try {
    await post({ method: 'monitor-report', kind, message: message.slice(0, 512) });
  } catch {
    // 监控是尽力而为；页面不因上报失败改变行为。
  }
}

/** 发消息（requestId 是幂等键：重发同一个 id 不会让 Kind 收到两次）。 */
/** T013.4：发消息时可选带粘贴段与附件引用（服务端拼成用户消息的块）。 */
export interface SendAttachmentRef {
  id: string;
  name: string;
  bytes: number;
}

export interface SendOptions {
  /** 用户粘贴的段（每段一个块）。 */
  pastes?: string[];
  /** 用户拖进来的文件（已存进附件仓库）。 */
  attachments?: SendAttachmentRef[];
}

export async function sendMessage(
  content: string,
  requestId: string,
  options: SendOptions = {},
): Promise<void> {
  await post({
    method: 'send',
    requestId,
    content,
    pastes: options.pastes ?? [],
    attachments: options.attachments ?? [],
  });
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
  const response = await post<{ root: ServiceRootState; host?: { home?: string } }>({
    method: 'recheck',
  });
  // T009.5：根不可用时页面没有 ready 帧；recheck 也带 host.home，说明页的路径照样缩写。
  setHostHome(response.host?.home);
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

/** 与 service 协议 v4 对应的 Keyring 类型（响应里没有 key 正文）。 */
export interface ServiceKeyFileInfo {
  path: string;
  since: number;
  problem?: string;
}

/** T012.4：连接的新形状（服务端 snake_case）。 */
export interface ServiceModelRow {
  id: string;
  name?: string;
  context_window?: number;
  max_tokens?: number;
  /** T012 F1：用户填的四类单价（美元/百万 token，不乘倍率；服务端 camelCase）。 */
  priceInput?: number;
  priceOutput?: number;
  priceCacheRead?: number;
  priceCacheWrite?: number;
}

export interface ServiceConnectionInfo {
  id: string;
  name: string;
  model: string;
  endpoint: string;
  saved_at: number;
  key_saved: boolean;
  /** T012.4：来源预设（deepseek/minimax/minimax-cn；空/未知 = 自定义）。 */
  preset?: string;
  /** 用户在这个连接里存的模型行（价格/窗口/输出上限）。 */
  models?: ServiceModelRow[];
  /** T012.4：价格倍率（1 = 原价，0 = 免费）。 */
  price_rate?: number;
  /** T012.6：学到的单头（x-api-key / authorization）。 */
  auth_header?: string | null;
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
  /** T012.4：预设来源、模型行、倍率（只增）。 */
  preset?: string;
  models?: ServiceModelRow[];
  price_rate?: number;
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

/** 选模型/思考档：下一轮生效，正在跑的轮不换（D26）。T012.9：可同时给连接/模型/想要的档。 */
export interface SessionModelSelectInput {
  connection?: string;
  model?: string;
  effortWanted?: string;
}
export async function selectSessionModel(
  input: SessionModelSelectInput,
): Promise<{ connection: string; model: string; effortWanted: string; effortActual: string }> {
  return post<{ connection: string; model: string; effortWanted: string; effortActual: string }>({
    method: 'session-model-select',
    ...input,
  });
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

/** T008.3/T012.4：改连接（endpoint/model 必填；preset/models/price_rate 只增），保留已存 key。 */
export async function updateConnectionInfo(
  id: string,
  patch: {
    endpoint: string;
    model: string;
    preset?: string;
    models?: ServiceModelRow[];
    price_rate?: number;
  },
): Promise<ServiceConnectionInfo> {
  const response = await post<{ connection: ServiceConnectionInfo }>({
    method: 'connection-update',
    id,
    ...patch,
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

/* ───────────────── T012.9：统计 / 模型目录 / 余额余量 / 设置 ───────────────── */

/** 会话统计（服务 status.stats；从日志现算，页面不自己加）。 */
export interface ServiceSessionStats {
  turns: number;
  steps: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  contextUsed: number;
  contextWindow: number;
  costUsd?: number;
  unpriced: boolean;
  connection: string;
  model: string;
  effortWanted: string;
  /** T012 F2：这一轮实际会发的档（服务端内核算好）。 */
  effortActual?: string;
}

export interface ServiceFxInfo {
  rate: number;
  fetchedAt: number;
}

export interface ServiceStatus {
  stats?: ServiceSessionStats;
  fx?: ServiceFxInfo;
  currency?: string;
}

export async function fetchStatus(): Promise<ServiceStatus> {
  return post<ServiceStatus>({ method: 'status' });
}

/** T012.9：写非密钥设置（目前只有 currency）；返回生效的币种。 */
export async function updateSettings(input: { currency?: string }): Promise<string> {
  const response = await post<{ currency: string }>({ method: 'settings-update', ...input });
  return response.currency;
}

/** 模型目录（服务把 models.dev 目录和厂商列表合并后给的；efforts 是这个模型自己的档）。 */
export interface ServiceCatalogModel {
  id: string;
  name?: string;
  contextWindow?: number;
  maxTokens?: number;
  /** 目录标价四类（服务端 ModelInfo 序列化是 snake_case）。 */
  price_input?: number;
  price_output?: number;
  price_cache_read?: number;
  price_cache_write?: number;
  thinks?: boolean;
  efforts?: string[];
}

export async function fetchAvailableModels(
  id: string,
): Promise<{ models: ServiceCatalogModel[]; live: boolean }> {
  return post<{ models: ServiceCatalogModel[]; live: boolean }>({ method: 'available-models', id });
}

/** T012.8b：连接余额（厂商币种）或套餐余量（MiniMax：5 小时窗口/每周剩余百分比）。 */
export interface ServiceQuota {
  balance?: { currency: string; amount: number };
  plan?: { intervalRemainingPercent?: number; weeklyRemainingPercent?: number };
  checkedAt: number;
}

export async function fetchConnectionQuota(id: string): Promise<ServiceQuota> {
  return post<ServiceQuota>({ method: 'connection-quota', id });
}
