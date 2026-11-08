// T007.3/4：页面与 aios-dev serve 的线上客户端（只加页面要用的方法/帧）。
// 协议事实：POST /api {method,...} → Response；WS /ws 先 ready 再事件/agent 帧。

import { apiErrorReason } from './errors';

/** T017.1：失败原因（原 data/fixtures/kindLive 的演示类型；事件与页面都在用）。 */
export type FailureCause = 'credential' | 'keyring' | 'model' | 'masked';

export interface ServiceEvent {
  seq: number;
  /** 日志事件发生时间（unix 毫秒；工作过程用）。 */
  time: number;
  type: string;
  data: Record<string, unknown>;
  cause?: FailureCause;
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
  type: 'text-delta' | 'usage' | 'retrying' | 'tool-call-delta';
  text?: string;
  /** T015.5：工具参数增量（不落日志）；callId/name/argumentsDelta。 */
  callId?: string;
  name?: string;
  argumentsDelta?: string;
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
  /** T015.4：画布按钮放进输入框的那一句带着画布身份（id + 标题）；自己写的不带。 */
  canvas?: { id: string; title: string };
}

/** T013.7：把一段文字文件存进附件仓库（base64）；二进制会拒收。 */
export interface AttachmentPutResult {
  id: string;
  name: string;
  bytes: number;
}

export async function attachmentPut(name: string, base64Data: string): Promise<AttachmentPutResult> {
  const response = await post({ method: 'attachment-put', name, data: base64Data });
  return response as unknown as AttachmentPutResult;
}

/** T013.7：按 id 读回附件（文字文件给全文；二进制 text 为空）。 */
export interface AttachmentGetResult {
  name: string;
  bytes: number;
  text?: string;
}

export async function attachmentGet(id: string): Promise<AttachmentGetResult> {
  const response = await post({ method: 'attachment-get', id });
  return response as unknown as AttachmentGetResult;
}

/** T013.7：按打的字列 @ 文件候选（服务端 BFS + 排序）。 */
export interface FileReferenceEntry {
  path: string;
  bytes: number;
}

export async function fileReferences(query: string): Promise<FileReferenceEntry[]> {
  const response = await post({ method: 'file-references-list', query });
  return (response as unknown as { references?: FileReferenceEntry[] }).references ?? [];
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
    // 没有标记就不带这个字段（服务端缺省就是用户自己写的）。
    ...(options.canvas ? { canvas: options.canvas } : {}),
  });
}

/** 取消当前轮（T007.5 停止按钮 / Esc；服务端取消正在跑的一轮并保留已做的）。 */
export async function cancelTurn(): Promise<void> {
  await post({ method: 'cancel' });
}

/** ask_user 的问题形状（T017.1：从 fixtures/kindLive 挪进正式数据模块）。 */
export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  id: string;
  header?: string;
  question: string;
  options?: AskOption[];
  multiSelect?: boolean;
}

/** question/answered 的回答形状（按问题 id）；线上回答用 ServiceAnswer（questionId）。 */
export interface AskAnswer {
  id: string;
  selected: string[];
  custom?: string;
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

/** 确认卡上的按钮：确认某一项，或者暂不。 */
export type CardVerb = 'confirm' | 'dismiss';

/** card/decision 事件的形状（服务在段尾回来一条；页面从日志事件也能收到同一条）。 */
export interface CardDecision {
  callId: string;
  option?: string;
  action?: string;
  phase: 'confirmed' | 'dismissed' | 'refused' | 'failed';
  data?: { summary?: string } & Record<string, unknown>;
}

export interface CardActResponse {
  decision?: CardDecision;
}

/** 点确认卡：服务跑核对和执行，记一条 card/decision；重复点击返回已经记下的那条。 */
export async function cardAct(
  callId: string,
  verb: CardVerb,
  option?: string,
): Promise<CardActResponse> {
  return post<CardActResponse>({
    method: 'card-act',
    callId,
    verb,
    ...(option ? { option } : {}),
  });
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

/** T012.4：连接里的一行模型（服务端 `credentials::ModelRow`，线上一律 camelCase）。 */
export interface ServiceModelRow {
  id: string;
  name?: string;
  contextWindow?: number;
  maxTokens?: number;
  /** T012 F1：用户填的四类单价（美元/百万 token；空=用目录标价）。 */
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

/** 一个连接存下的厂商模型列表（页面小标签用）。 */
export interface ServiceListedModels {
  fetchedAt: number;
  models: ServiceCatalogModel[];
}

/** 设置页响应：Keyring 视图（旧字段）+ 屏蔽表（只增字段）。 */
export interface ServiceModelSettings {
  settings: ServiceKeyringView;
  disabledModels: string[];
  disabledUnknown: boolean;
  /** 与 disabledModels 等长同序：每条规则现在命中的「连接/模型」。 */
  disabledHits: string[][];
  /** 每个连接现在存下的厂商列表，键是连接 id。 */
  listed: Record<string, ServiceListedModels>;
}

export interface SaveConnectionInput {
  id: string;
  name: string;
  model: string;
  endpoint: string;
  apiKey: string;
  /** T012.4：预设来源、模型行（只增）。 */
  preset?: string;
  models?: ServiceModelRow[];
}

/** 设置页读取（不含 key 正文）。 */
export async function fetchModelSettings(): Promise<ServiceModelSettings> {
  return post<ServiceModelSettings>({ method: 'model-settings' });
}

/** 整表替换屏蔽规则；返回规范化后的整表（去空白/空规则/不分大小写去重，保持顺序）。 */
export async function setDisabledModels(models: string[]): Promise<string[]> {
  const response = await post<{ models: string[] }>({ method: 'disabled-models-set', models });
  return response.models;
}

/** 谁在用：当前 Kind 对话（带是否在跑）与新会话（上一次选的）。 */
export type DisabledModelUser =
  | { kind: 'kind-chat'; running: boolean }
  | { kind: 'new-sessions' };

/** 一条规则现在匹配哪些模型、谁在用（确认框用；页面不自己匹配）。 */
export interface DisabledModelsCheck {
  hits: string[];
  users: DisabledModelUser[];
}

export async function checkDisabledModels(pattern: string): Promise<DisabledModelsCheck> {
  return post<DisabledModelsCheck>({ method: 'disabled-models-check', pattern });
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

/** T008.3/T012.4：改连接（endpoint/model 必填；preset/models 只增），保留已存 key。 */
export async function updateConnectionInfo(
  id: string,
  patch: {
    endpoint: string;
    model: string;
    preset?: string;
    models?: ServiceModelRow[];
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
  /** 会话的工作目录（页面把工作过程里的路径显示成相对路径；旧服务的 status 没有它）。 */
  cwd?: string;
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
  /** 和上一次存下的厂商列表比没见过（第一次获取的基准在服务端定）。 */
  fresh?: boolean;
  /** 目录标价四类（服务端 ModelInfo 是 camelCase）。 */
  priceInput?: number;
  priceOutput?: number;
  priceCacheRead?: number;
  priceCacheWrite?: number;
  thinks?: boolean;
  efforts?: string[];
}

/** 只看一眼可用模型（打开选择器、补价格/窗口/档）：服务端不落盘，「新」的基准也不动。 */
export async function peekAvailableModels(
  id: string,
): Promise<{ models: ServiceCatalogModel[]; live: boolean }> {
  return post<{ models: ServiceCatalogModel[]; live: boolean }>({
    method: 'available-models',
    id,
    mode: 'query',
  });
}

/** 用户按了「获取可用模型」：厂商这次的列表由服务端存下，下次拿它当「新」的基准。 */
export async function discoverAvailableModels(
  id: string,
): Promise<{ models: ServiceCatalogModel[]; live: boolean }> {
  return post<{ models: ServiceCatalogModel[]; live: boolean }>({
    method: 'available-models',
    id,
    mode: 'fetch',
  });
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
