// T007.3/4：页面与 aios-dev serve 的线上客户端（只加页面要用的方法/帧）。
// 协议事实：POST /api {method,...} → Response；WS /ws 先 ready 再事件/agent 帧。
// T025：线上类型由 Rust 生成（crates/service/src/protocol.rs、crates/web/src/connection.rs），
// 生成物在 ./generated/；这里只留页面自己的输入形状和请求胶水，不再手写协议字段。

import { apiErrorReason } from './errors';
import type { AttachmentGetResponse } from './generated/AttachmentGetResponse';
import type { AttachmentPutResponse } from './generated/AttachmentPutResponse';
import type { AvailableModelsResponse } from './generated/AvailableModelsResponse';
import type { Call } from './generated/Call';
import type { CardActResponse } from './generated/CardActResponse';
import type { CardVerb } from './generated/CardVerb';
import type { ConnectionInfo } from './generated/ConnectionInfo';
import type { ConnectionQuotaResponse } from './generated/ConnectionQuotaResponse';
import type { ConnectionSaveRequest } from './generated/ConnectionSaveRequest';
import type { ConnectionUpdateRequest } from './generated/ConnectionUpdateRequest';
import type { DisabledModelsCheckResponse } from './generated/DisabledModelsCheckResponse';
import type { EventsResponse } from './generated/EventsResponse';
import type { FileReferenceEntry } from './generated/FileReferenceEntry';
import type { FirstUseResponse } from './generated/FirstUseResponse';
import type { Frame } from './generated/Frame';
import type { IssuesResponse } from './generated/IssuesResponse';
import type { KeyringView } from './generated/KeyringView';
import type { ModelSettingsResponse } from './generated/ModelSettingsResponse';
import type { Response } from './generated/Response';
import type { RootState } from './generated/RootState';
import type { ServiceAnswer } from './generated/ServiceAnswer';
import type { SessionModelSelectRequest } from './generated/SessionModelSelectRequest';
import type { SessionModelSelectResponse } from './generated/SessionModelSelectResponse';
import type { ExecSettingsView } from './generated/ExecSettingsView';
import type { SettingsUpdateRequest } from './generated/SettingsUpdateRequest';
import type { SettingsUpdateResponse } from './generated/SettingsUpdateResponse';
import type { StatusResponse } from './generated/StatusResponse';
import type { StopMode } from './generated/StopMode';
import type { TaskInterruptResponse } from './generated/TaskInterruptResponse';
import type { TaskSayResponse } from './generated/TaskSayResponse';
import type { TaskStopResponse } from './generated/TaskStopResponse';
import type { ToolKeyView } from './generated/ToolKeyView';

// 页面从这里按名字取线上类型；类型本体只有 Rust 一份。
export type { AvailableModelRow } from './generated/AvailableModelRow';
export type { CardDecision } from './generated/CardDecision';
export type { DisabledModelUser } from './generated/DisabledModelUser';
export type { FailureCause } from './generated/FailureCause';
export type { KeyFileInfo } from './generated/KeyFileInfo';
export type { ModelRow } from './generated/ModelRow';
export type { RootState } from './generated/RootState';
export type { ServiceEvent } from './generated/ServiceEvent';
export type { CardVerb };
export type { ConnectionInfo };
export type { ConnectionQuotaResponse };
export type { ExecSettingsView };
export type { FileReferenceEntry };
export type { Frame };
export type { StopMode };
export type { TaskStopResponse };
export type { KeyringView };
export type { ModelSettingsResponse };
export type { ServiceAnswer };
export type { StatusResponse };
export type { ToolKeyView };

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
  /** 服务端 error body 的 message（给「原因」这类要带事实的降级文案用）。 */
  detail?: string;
  constructor(status: number, message: string, code?: string, detail?: string) {
    super(message);
    this.status = status;
    this.code = code;
    this.detail = detail;
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

/** 按方法从生成的调用面取请求/返回；方法名或字段写错，tsc 直接报。 */
type CallOf<M extends Call['method']> = Extract<Call, { method: M }>;
type ResponseOf<M extends Call['method']> = Extract<Response, { method: M }>;

async function post<M extends Call['method']>(body: CallOf<M>): Promise<ResponseOf<M>> {
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
    let detail: string | undefined;
    try {
      const body = (await response.json()) as { code?: unknown; message?: unknown };
      if (typeof body.code === 'string') code = body.code;
      if (typeof body.message === 'string') detail = body.message;
    } catch {
      // 非 JSON 错误体：走通用原因。
    }
    throw new ServiceHttpError(response.status, apiErrorReason(code), code, detail);
  }
  // 线上入口的断言之一：响应体就是 Response 里这个方法的分支。
  return (await response.json()) as ResponseOf<M>;
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
export async function attachmentPut(name: string, base64Data: string): Promise<AttachmentPutResponse> {
  return post({ method: 'attachment-put', name, data: base64Data });
}

/** T013.7：按 id 读回附件（文字文件给全文；二进制 text 为空）。 */
export async function attachmentGet(id: string): Promise<AttachmentGetResponse> {
  return post({ method: 'attachment-get', id });
}

/** T013.7：按打的字列 @ 文件候选（服务端 BFS + 排序）。 */
export async function fileReferences(query: string): Promise<FileReferenceEntry[]> {
  const response = await post({ method: 'file-references-list', query });
  return response.references;
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
    // 服务端缺省就是 follow-up；显式给，生成的请求类型要求这个字段。
    delivery: 'follow-up',
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

/** question/answered 的回答形状（按问题 id）；线上回答用 ServiceAnswer（questionId）。 */
export interface AskAnswer {
  id: string;
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

/** 点确认卡：服务跑核对和执行，记一条 card/decision；重复点击返回已经记下的那条。 */
export async function cardAct(
  callId: string,
  verb: CardVerb,
  option?: string,
): Promise<CardActResponse> {
  return post({
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

export async function recheckRoot(): Promise<RootState> {
  const response = await post({ method: 'recheck' });
  // T009.5：根不可用时页面没有 ready 帧；recheck 也带 host.home，说明页的路径照样缩写。
  setHostHome(response.host.home);
  return response.root;
}

/** task-read：任务对话的头部 + 一页事件（只读；beforeSeq 往回翻，省略 = 日志末端）。 */
export async function taskRead(taskId: string, beforeSeq?: number) {
  return post({
    method: 'task-read',
    taskId,
    ...(beforeSeq === undefined ? {} : { beforeSeq }),
  });
}

/** 停一个任务；Kind 的 task_stop 走同一个服务方法。 */
export async function taskStop(taskId: string, mode: StopMode): Promise<TaskStopResponse> {
  return post({ method: 'task-stop', taskId, mode });
}

/** T028.5：任务对话里的粘贴段与附件（和 Kind 对话的 SendOptions 同名同形）。 */
export type TaskSayOptions = Pick<SendOptions, 'pastes' | 'attachments'>;

/**
 * T028.5：在任务对话里说一句。在跑时插进这一轮；停了/失败了就在原会话开新 Attempt。
 * 全局暂停时服务端先把这句话排着（响应照收）。
 */
export async function taskSay(
  taskId: string,
  text: string,
  options: TaskSayOptions = {},
): Promise<TaskSayResponse> {
  const pastes = options.pastes ?? [];
  const attachments = options.attachments ?? [];
  return post({
    method: 'task-say',
    taskId,
    text,
    ...(pastes.length > 0 ? { pastes } : {}),
    ...(attachments.length > 0 ? { attachments } : {}),
  });
}

/** T028.5：停下这一轮（不结束 Attempt，不出卡）；Esc 和输入框旁的停止都走这里。 */
export async function taskInterrupt(taskId: string): Promise<TaskInterruptResponse> {
  return post({ method: 'task-interrupt', taskId });
}

/** events：往回翻一页日志（T006.16c 的轮对齐预算；打开/往上翻用）。 */
export async function fetchEventsPage(options: {
  beforeSeq?: number;
  maxMessages?: number;
  minMessages?: number;
  minTurns?: number;
}): Promise<EventsResponse> {
  return post({ method: 'events', ...options });
}

/* ───────────────────────── T008 设置 › 模型与连接（Keyring） ───────────────────────── */

/** 设置页读取（不含 key 正文）。 */
export async function fetchModelSettings(): Promise<ModelSettingsResponse> {
  return post({ method: 'model-settings' });
}

/** 整表替换屏蔽规则；返回规范化后的整表（去空白/空规则/不分大小写去重，保持顺序）。 */
export async function setDisabledModels(models: string[]): Promise<string[]> {
  const response = await post({ method: 'disabled-models-set', models });
  return response.models;
}

/** 一条规则现在匹配哪些模型、谁在用（确认框用；页面不自己匹配）。 */
export async function checkDisabledModels(pattern: string): Promise<DisabledModelsCheckResponse> {
  return post({ method: 'disabled-models-check', pattern });
}

/** 存/换连接；key 只在这一次请求体里。 */
export async function saveConnection(input: ConnectionSaveRequest): Promise<ConnectionInfo> {
  const response = await post({ method: 'connection-save', ...input });
  return response.connection;
}

export async function deleteConnection(id: string): Promise<boolean> {
  const response = await post({ method: 'connection-delete', id });
  return response.deleted;
}

/** 验证连接；结果落在注册器（响应不带 key）。 */
export async function verifyConnection(id: string): Promise<ConnectionInfo> {
  const response = await post({ method: 'connection-verify', id });
  return response.connection;
}

export async function relocateKeyring(to: string): Promise<void> {
  await post({ method: 'keyring-relocate', to });
}

export async function reopenKeyring(at: string): Promise<void> {
  await post({ method: 'keyring-reopen', at });
}

export async function restartKeyring(): Promise<KeyringView> {
  const response = await post({ method: 'keyring-restart' });
  return response.settings;
}

/* ───────────────── 设置页「联网搜索」：Exa 凭证（D73） ───────────────── */

/** Exa 凭证的线上视图：存没存、保存时间、上次验证结果。没有 key 正文。 */
export async function fetchExaKey(): Promise<ToolKeyView> {
  return post({ method: 'exa-key' });
}

/** 存/换 Exa 的 key；key 只在这一次请求体里，响应不带。 */
export async function saveExaKey(key: string): Promise<ToolKeyView> {
  return post({ method: 'exa-key-save', key });
}

/** 用已存的 key 向 Exa 发一次最小搜索，结果写回 Keyring。 */
export async function checkExaKey(): Promise<ToolKeyView> {
  return post({ method: 'exa-key-check' });
}

export async function deleteExaKey(): Promise<ToolKeyView> {
  return post({ method: 'exa-key-delete' });
}

export async function fetchIssues(): Promise<IssuesResponse> {
  return post({ method: 'issues' });
}

export async function fetchFirstUse(): Promise<FirstUseResponse> {
  return post({ method: 'first-use' });
}

/** 选模型/思考档：下一轮生效，正在跑的轮不换（D26）。T012.9：可同时给连接/模型/想要的档。 */
export async function selectSessionModel(
  input: SessionModelSelectRequest,
): Promise<SessionModelSelectResponse> {
  return post({ method: 'session-model-select', ...input });
}

/** 连 WS 的本体：ready → 补齐/直播的日志事件（event）与不落日志的 agent 帧（流式文本等）。 */
function openSocket(
  path: string,
  onFrame: (frame: Frame) => void,
  onClose: () => void,
): WebSocket {
  const target = serviceTarget();
  const socket = new WebSocket(target.origin.replace('http', 'ws') + path);
  socket.onmessage = (message) => {
    try {
      // 线上入口的断言之二：WS 帧就是生成的 Frame。
      onFrame(JSON.parse(String(message.data)) as Frame);
    } catch {
      // 坏帧不打断页面；真正缺事件由重连补齐。
    }
  };
  socket.onclose = onClose;
  socket.onerror = onClose;
  return socket;
}

/** 连根会话：Kind 对话。 */
export function connectSession(
  lastSeq: number,
  onFrame: (frame: Frame) => void,
  onClose: () => void,
): WebSocket | null {
  return openSocket(`/ws?lastSeq=${lastSeq}`, onFrame, onClose);
}

/** 连任务对话：服务端把它接到这个任务当前 Attempt 的会话；没在跑时只补齐。 */
export function connectTaskSession(
  taskId: string,
  lastSeq: number,
  onFrame: (frame: Frame) => void,
  onClose: () => void,
): WebSocket {
  return openSocket(
    `/ws?task=${encodeURIComponent(taskId)}&lastSeq=${lastSeq}`,
    onFrame,
    onClose,
  );
}

/** T008.3/T012.4：改连接（endpoint/model 必填；preset/models 只增），保留已存 key。 */
export async function updateConnectionInfo(
  id: string,
  patch: Omit<ConnectionUpdateRequest, 'id'>,
): Promise<ConnectionInfo> {
  const response = await post({ method: 'connection-update', id, ...patch });
  return response.connection;
}

/** T008.3 增补：撤销 key（连接保留）。 */
export async function revokeConnectionKey(id: string): Promise<ConnectionInfo> {
  const response = await post({ method: 'connection-revoke', id });
  return response.connection;
}

/* ───────────────── T012.9：统计 / 模型目录 / 余额余量 / 设置 ───────────────── */

export async function fetchStatus(): Promise<StatusResponse> {
  return post({ method: 'status' });
}

/** T012.9：写非密钥设置；返回生效的值（币种 + 执行设置）。 */
export async function updateSettings(input: SettingsUpdateRequest): Promise<SettingsUpdateResponse> {
  return post({ method: 'settings-update', ...input });
}

/** 设置 › 执行：读当前值（全空请求只读，不写文件）。 */
export async function loadExecSettings(): Promise<ExecSettingsView> {
  return (await post({ method: 'settings-update' })).exec;
}

/** 设置 › 执行：只改给出的项；返回生效后的值。 */
export async function saveExecSettings(patch: {
  paused?: boolean;
  seats?: number;
}): Promise<ExecSettingsView> {
  return (await post({ method: 'settings-update', ...patch })).exec;
}

/** 只看一眼可用模型（打开选择器、补价格/窗口/档）：服务端不落盘，「新」的基准也不动。 */
export async function peekAvailableModels(id: string): Promise<AvailableModelsResponse> {
  return post({ method: 'available-models', id, mode: 'query' });
}

/** 用户按了「获取可用模型」：厂商这次的列表由服务端存下，下次拿它当「新」的基准。 */
export async function discoverAvailableModels(id: string): Promise<AvailableModelsResponse> {
  return post({ method: 'available-models', id, mode: 'fetch' });
}

/** T012.8b：连接余额（厂商币种）或套餐余量（MiniMax：5 小时窗口/每周剩余百分比）。 */
export async function fetchConnectionQuota(id: string): Promise<ConnectionQuotaResponse> {
  return post({ method: 'connection-quota', id });
}
