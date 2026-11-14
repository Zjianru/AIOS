/**
 * src/data/api.ts —— 界面取数据的唯一入口（需求 §二）。
 *
 * - 形状照真实 RPC：threadResume / threadTurnsList（见 types.ts 头部依据），字段 camelCase。
 * - 每次调用都把返回值 JSON.stringify 再 JSON.parse 一遍，模拟传输与解析的成本。
 * - 延迟可调，默认 40ms；设置里有开发开关可调到 0 或 300。
 * - cursor 是不透明字符串；nextCursor 为空表示到头了。
 * - 组件里不许直接 import 样例数据，只能从这里取。
 */

import { kindSource, taskRunSource, getKindLog, inProgressProjects } from './generate';
import type { TurnSource } from './generate';
import { abilities, builtinFlows, flowIndex } from './fixtures/abilities';
import type { BuiltinFlow } from './fixtures/abilities';
import { kindContext } from './fixtures/context';
import type { KindContextFixture } from './fixtures/context';
import { COMPACTION_SUMMARY, DEMO_SCRIPTS, defaultScript } from './fixtures/kindLive';
import type { LiveDemo, LiveScript } from './fixtures/kindLive';
import { DISCOVERED_MODELS, modelSettingsFixture } from './fixtures/settings';
import type { ConnectionView, ModelRow, ModelSettingsView, Protocol } from './keyring';
import { projectDetail, readDoc, taskDetail } from './selectors';
import type { ProjectDetail, ReadDoc, TaskDetail } from './selectors';
import { NEW_FLOW_DRAFTS, PALETTE_PARTS, SEED_FLOWS, TEMPLATE_OPTIONS, makeStressFlow } from './flows';
import { artifacts, materials, projects, roles, tasks } from './fixtures/workspace';
import type { ArtifactFixture, MaterialFixture, ProjectFixture, RoleFixture, TaskFixture } from './fixtures/workspace';
import type {
  ItemDelta, SortDirection, ThreadResumeParams, ThreadResumeResponse, ThreadTurnsListParams,
  TurnsListResponse, Turn, TurnItemsView, TurnsPage,
} from './types';

/* ───────────────────────── 传输模拟 ───────────────────────── */

export type MockLatency = 0 | 40 | 300;
let latencyMs: MockLatency = 40;

/** 模拟走一遍网络：序列化 + 反序列化，再按设定延迟等一等。 */
function transmit<T>(value: T): Promise<T> {
  const wire = JSON.stringify(value);
  const parsed = JSON.parse(wire) as T;
  return new Promise((resolve) => {
    if (latencyMs === 0) resolve(parsed);
    else setTimeout(() => resolve(parsed), latencyMs);
  });
}

/* ───────────────────────── 不透明 cursor ───────────────────────── */

const CURSOR_PREFIX = 'v14c';

function encodeCursor(index: number): string {
  return CURSOR_PREFIX + '.' + index.toString(36);
}

function decodeCursor(cursor: string | null | undefined): number | null {
  if (!cursor) return null;
  const body = cursor.startsWith(CURSOR_PREFIX + '.') ? cursor.slice(CURSOR_PREFIX.length + 1) : cursor;
  const n = parseInt(body, 36);
  return Number.isFinite(n) ? n : null;
}

/* ───────────────────────── 追加的轮（流式用） ───────────────────────── */

const appendedTurns = new Map<string, Turn[]>();

function totalTurnsOf(source: TurnSource): number {
  return source.totalTurns + (appendedTurns.get(source.thread.id)?.length ?? 0);
}

function turnAt(source: TurnSource, index: number): Turn {
  const extra = appendedTurns.get(source.thread.id);
  if (extra && index >= source.totalTurns) return extra[index - source.totalTurns];
  return source.turnAt(index);
}

function sourceFor(threadId: string): TurnSource | null {
  if (threadId === kindSource.thread.id) return kindSource;
  if (threadId === taskRunSource.thread.id) return taskRunSource;
  return null;
}

/* ───────────────────────── thread/resume + thread/turns/list ───────────────────────── */

interface PageParams {
  cursor?: string | null;
  limit: number;
  sortDirection: SortDirection;
  itemsView: TurnItemsView;
}

function buildPage(source: TurnSource, params: PageParams): TurnsPage {
  const total = totalTurnsOf(source);
  const descending = params.sortDirection === 'desc';
  const startRaw = decodeCursor(params.cursor);
  const start = startRaw === null ? (descending ? total - 1 : 0) : startRaw;
  const step = descending ? -1 : 1;
  const data: Turn[] = [];
  let index = start;
  while (data.length < params.limit && index >= 0 && index < total) {
    data.push(turnAt(source, index));
    index += step;
  }
  const last = index - step;
  const nextIndex = index;
  const hasNext = nextIndex >= 0 && nextIndex < total;
  const backwardIndex = start - step;
  const hasBackward = backwardIndex >= 0 && backwardIndex < total && data.length > 0;
  void last;
  return {
    data,
    nextCursor: hasNext ? encodeCursor(nextIndex) : null,
    backwardsCursor: hasBackward ? encodeCursor(backwardIndex) : null,
  };
}

/** 协议：thread/resume。excludeTurns: true 时 thread 不带 turns，只给首页。 */
export async function threadResume(params: ThreadResumeParams): Promise<ThreadResumeResponse> {
  const source = sourceFor(params.threadId);
  if (!source) throw new Error('threadResume: 没有这个会话 ' + params.threadId);
  const initial = params.initialTurnsPage;
  const page = buildPage(source, {
    limit: initial?.limit ?? 5,
    sortDirection: initial?.sortDirection ?? 'desc',
    itemsView: initial?.itemsView ?? 'full',
  });
  return transmit({
    thread: source.thread,
    model: source.thread.model,
    modelProvider: source.thread.modelProvider,
    reasoningEffort: 'medium',
    initialTurnsPage: page,
  });
}

/** 协议：thread/turns/list。desc 取到的是「越往前越早」的页。 */
export async function threadTurnsList(params: ThreadTurnsListParams): Promise<TurnsListResponse> {
  const source = sourceFor(params.threadId);
  if (!source) throw new Error('threadTurnsList: 没有这个会话 ' + params.threadId);
  const page = buildPage(source, {
    cursor: params.cursor ?? null,
    limit: params.limit ?? 20,
    sortDirection: params.sortDirection ?? 'desc',
    itemsView: params.itemsView ?? 'full',
  });
  return transmit(page);
}

/* ───────────────────────── 流式回复（本地事件模拟） ───────────────────────── */

type DeltaListener = (delta: ItemDelta) => void;
const deltaListeners = new Set<DeltaListener>();

/** 订阅增量事件。取消订阅 = 调用返回的函数。 */
export function subscribeItemDelta(listener: DeltaListener): () => void {
  deltaListeners.add(listener);
  return () => {
    deltaListeners.delete(listener);
  };
}

function emit(delta: ItemDelta): void {
  for (const listener of deltaListeners) listener(delta);
}

export interface MockStreamHandle {
  stop(): void;
  turnId: string;
  itemId: string;
}

const STREAM_REPLY = [
  '看过了。这一段的结论没变，但有两处要补：',
  '',
  '1. 多引擎实测的命中率，去重之后要从 21 / 20 改成 18 / 20；',
  '2. 缺 Bing 这一项要单独标出来，不要并进总计。',
  '',
  '补完这两处，评测记录.md 就是可以验收的 v3 了。另外可行性验证已经到场景 3，剩下的场景 4 预计 20 分钟。',
];

/**
 * 未核实：真实接口里「流式回复」的事件名与粒度。这里是本地模拟，
 * 事件名先按 item/started、item/delta、item/completed、turn/completed 写，接真接口时对齐。
 */
export function startMockReply(options: { threadId: string; totalMs?: number; text?: string }): MockStreamHandle {
  const source = sourceFor(options.threadId);
  if (!source) throw new Error('startMockReply: 没有这个会话');
  const total = options.totalMs ?? 30000;
  const text = options.text ?? STREAM_REPLY.join(String.fromCharCode(10));
  const turnIndex = totalTurnsOf(source);
  const turnId = 'turn_live_' + turnIndex;
  const itemId = 'item_live_' + turnIndex;
  const turn: Turn = {
    id: turnId,
    items: [{ type: 'agentMessage', id: itemId, text: '', phase: 'final_answer' }],
    itemsView: 'full',
    status: 'inProgress',
    error: null,
    startedAt: Math.floor(Date.now() / 1000),
    completedAt: null,
    durationMs: null,
  };
  const list = appendedTurns.get(source.thread.id) ?? [];
  list.push(turn);
  appendedTurns.set(source.thread.id, list);
  emit({ type: 'item/started', threadId: source.thread.id, turnId, itemId });

  const chunk = Math.max(1, Math.ceil(text.length / 120));
  const tickMs = Math.max(16, Math.round(total / (text.length / chunk)));
  let offset = 0;
  const timer = setInterval(() => {
    const next = Math.min(text.length, offset + chunk);
    const delta = text.slice(offset, next);
    offset = next;
    const item = turn.items[0];
    if (item.type === 'agentMessage') item.text = text.slice(0, offset);
    emit({ type: 'item/delta', threadId: source.thread.id, turnId, itemId, delta });
    if (offset >= text.length) {
      clearInterval(timer);
      turn.status = 'completed';
      turn.completedAt = Math.floor(Date.now() / 1000);
      turn.durationMs = total;
      emit({ type: 'item/completed', threadId: source.thread.id, turnId, itemId });
      emit({ type: 'turn/completed', threadId: source.thread.id, turnId, itemId });
    }
  }, tickMs);

  return {
    stop() {
      clearInterval(timer);
    },
    turnId,
    itemId,
  };
}

/* ───────────────────────── 其余只读对象（第 2、3 期用，这一期先建好） ───────────────────────── */

export async function listProjects(): Promise<Record<string, ProjectFixture>> {
  return transmit(projects);
}

export async function listTasks(): Promise<Record<string, TaskFixture>> {
  return transmit(tasks);
}

export async function listRoles(): Promise<RoleFixture[]> {
  return transmit(roles);
}

export async function listMaterials(): Promise<MaterialFixture[]> {
  return transmit(materials);
}

export async function listArtifacts(): Promise<Record<string, ArtifactFixture>> {
  return transmit(artifacts);
}

export async function listKindLog() {
  return transmit(getKindLog());
}

/** Kind 右栏「在做的事」：三个项目的根任务（地铁线小图）。 */
export async function listInProgressTasks(): Promise<TaskFixture[]> {
  return transmit(inProgressProjects());
}

/** Kind 右栏「Kind 现在带着」：上下文由什么组成（§6.2）。 */
export async function getKindContext(): Promise<KindContextFixture> {
  return transmit(kindContext);
}

/** 首次使用的 Kind（§四）：提供方 / 三张起手卡片 / 推荐。 */
/* ───────────────────────── 设置 › 模型与连接（Keyring） ───────────────────────── */

/**
 * 有状态的假 Keyring：存、撤销、换钥匙位置。
 * 密钥正文不留在这里——只记「这把会不会被模拟拒绝」，返回值里永远没有正文。
 * 原型约定：以 sk-bad 开头的密钥模拟被 DeepSeek 拒绝。
 */
let keyring: ModelSettingsView = structuredClone(modelSettingsFixture);
/** 哪些连接的密钥会被模拟拒绝（按连接记，不记正文）。 */
const keyWillFail = new Set<string>();

function nowLabel(): string {
  const d = new Date();
  return '今天 ' + String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0');
}

function markKey(connectionId: string, key: string): void {
  if (key.trim().startsWith('sk-bad')) keyWillFail.add(connectionId);
  else keyWillFail.delete(connectionId);
}

function findConnection(id: string) {
  const conn = keyring.connections.find((item) => item.id === id);
  if (!conn) throw new Error('没有这个连接：' + id);
  return conn;
}

export async function getModelSettings(): Promise<ModelSettingsView> {
  return transmit(keyring);
}

export interface ConnectionDraft {
  source: 'catalog' | 'custom';
  /** catalog 时是目录里的 id；custom 时是用户填的 Provider ID。 */
  id: string;
  name: string;
  protocol: Protocol;
  endpoint: string;
  models: ModelRow[];
  /** 可以空：空就是先建连接、之后再填密钥。 */
  key: string;
}

const PROVIDER_ID = /^[a-z][a-z0-9-]*$/;

/** 新建一个连接；有密钥就同时存进 Keyring，并按假厂商的列表记一次检查结果。 */
export async function addConnection(draft: ConnectionDraft): Promise<ModelSettingsView> {
  if (!PROVIDER_ID.test(draft.id)) throw new Error('Provider ID 要以小写字母开头，只用小写字母、数字和 -');
  if (keyring.connections.some((item) => item.id === draft.id)) throw new Error('已经有一个叫 ' + draft.id + ' 的连接');
  if (draft.source === 'custom') {
    if (!/^https?:\/\/[^\s/]+/.test(draft.endpoint.trim())) throw new Error('API 地址要是 http 或 https 开头的完整地址');
    if (!draft.models.some((row) => row.id.trim())) throw new Error('自定义接口至少要写一个模型');
  }
  const catalog = keyring.catalog.find((item) => item.id === draft.id);
  const conn: ConnectionView = {
    id: draft.id,
    name: draft.name.trim() || catalog?.name || draft.id,
    source: draft.source,
    protocol: draft.protocol,
    endpoint: draft.endpoint.trim(),
    defaultEndpoint: catalog?.defaultEndpoint ?? draft.endpoint.trim(),
    models: draft.models.filter((row) => row.id.trim()),
    key: { saved: false },
    check: { kind: 'never' },
  };
  keyring.connections.push(conn);
  if (draft.key.trim()) {
    markKey(conn.id, draft.key);
    conn.key = { saved: true, savedAt: nowLabel() };
    conn.check = keyWillFail.has(conn.id)
      ? { kind: 'fail', at: nowLabel(), reason: conn.name + ' 拒绝了这把密钥（401）' }
      : { kind: 'ok', at: nowLabel() };
    conn.availableModels = catalog?.defaultModels ?? conn.models;
  }
  return transmit(keyring);
}

/** 改地址、模型目录；密钥有填就换一把（留空就是不动已保存的那一把）。 */
export async function updateConnection(
  connectionId: string,
  patch: { endpoint: string; models: ModelRow[]; key: string },
): Promise<ModelSettingsView> {
  const conn = findConnection(connectionId);
  const endpoint = patch.endpoint.trim();
  if (endpoint && !/^https?:\/\/[^\s/]+/.test(endpoint)) throw new Error('API 地址要是 http 或 https 开头的完整地址');
  conn.endpoint = endpoint;
  conn.models = patch.models.filter((row) => row.id.trim());
  if (patch.key.trim()) {
    markKey(conn.id, patch.key);
    conn.key = { saved: true, savedAt: nowLabel() };
    conn.check = keyWillFail.has(conn.id)
      ? { kind: 'fail', at: nowLabel(), reason: conn.name + ' 拒绝了这把密钥（401）' }
      : { kind: 'ok', at: nowLabel() };
    const catalog = keyring.catalog.find((item) => item.id === conn.id);
    conn.availableModels = catalog?.defaultModels ?? conn.models;
  }
  return transmit(keyring);
}

/** 删掉一个连接：它在 Keyring 里的密钥一起撤销。 */
export async function removeConnection(connectionId: string): Promise<ModelSettingsView> {
  findConnection(connectionId);
  keyring.connections = keyring.connections.filter((item) => item.id !== connectionId);
  keyWillFail.delete(connectionId);
  return transmit(keyring);
}

/** 「获取可用模型」：问一下那个地址有哪些模型。只回候选，不写进目录。 */
export async function discoverModels(target: { catalogId?: string; endpoint: string }): Promise<ModelRow[]> {
  if (!target.catalogId && !/^https?:\/\/[^\s/]+/.test(target.endpoint.trim())) {
    throw new Error('先填一个完整的 API 地址');
  }
  await new Promise((resolve) => setTimeout(resolve, 600));
  return transmit(DISCOVERED_MODELS[target.catalogId ?? 'custom'] ?? DISCOVERED_MODELS.custom);
}

/** 撤销：立即生效，之后的请求不再签发。 */
export async function revokeKey(connectionId: string): Promise<ModelSettingsView> {
  const conn = findConnection(connectionId);
  conn.key = { saved: false };
  conn.check = { kind: 'never' };
  keyWillFail.delete(connectionId);
  return transmit(keyring);
}

/** 把加密钥匙挪到用户指定的位置。 */
export async function moveKeyFile(path: string): Promise<ModelSettingsView> {
  const clean = path.trim();
  if (!clean.startsWith('/') && !clean.startsWith('~/')) throw new Error('要写完整路径，以 / 或 ~/ 开头');
  if (clean.endsWith('/')) throw new Error('要写到文件名，不是目录');
  keyring.keyFile = { path: clean, since: nowLabel() };
  return transmit(keyring);
}

/** 用户把钥匙文件挪走过：告诉 AIOS 它现在在哪。不搬文件，只改指向。 */
export async function relinkKeyFile(path: string): Promise<ModelSettingsView> {
  const clean = path.trim();
  if (!clean.startsWith('/') && !clean.startsWith('~/')) throw new Error('要写完整路径，以 / 或 ~/ 开头');
  await new Promise((resolve) => setTimeout(resolve, 400));
  keyring.keyFile = { path: clean, since: nowLabel() };
  return transmit(keyring);
}

/** 找不回旧钥匙：换一把新的。旧钥匙加密的密钥读不回来，所有连接都要重新填密钥。 */
export async function resetKeyring(): Promise<ModelSettingsView> {
  keyring.keyFile = { path: keyring.keyFile.path, since: nowLabel() };
  for (const conn of keyring.connections) {
    conn.key = { saved: false };
    conn.check = { kind: 'never' };
  }
  keyWillFail.clear();
  return transmit(keyring);
}

/* ───────────────────────── Kind 对话接内核后的演示（T007、T008 的参照） ───────────────────────── */

/** 平常发一条消息时，这一轮怎么走（假的；真的是内核推过来的事件）。 */
export function liveScriptFor(text: string): LiveScript {
  return structuredClone(defaultScript(text));
}

export function liveDemo(name: LiveDemo): LiveScript {
  return structuredClone(DEMO_SCRIPTS[name]);
}

export function compactionSummary(): string {
  return COMPACTION_SUMMARY;
}

/** 阅读页（§8.6）：资料与成果共用一份；按类型给渲染方式。 */
export async function getRead(fileName: string): Promise<ReadDoc | null> {
  return transmit(readDoc(fileName));
}

/** 项目页（§4.5 里程碑 / §4.6 成果）。 */
export async function getProject(projectId: string): Promise<ProjectDetail | null> {
  return transmit(projectDetail(projectId));
}

/** 任务页（§4.1–4.5）：站、节点层、账、里程碑。 */
export async function getTaskDetail(taskId: string): Promise<TaskDetail | null> {
  return transmit(taskDetail(taskId));
}

/* ───────────────────────── 原始记录（第三层，第 2 期） ───────────────────────── */

/** 一件任务的运行记录会话 id（样例是改后的 500 轮）。 */
export const TASK_RUN_THREAD_ID = taskRunSource.thread.id;
/** 样例里一共多少轮。 */
export const TASK_RUN_TOTAL_TURNS = taskRunSource.totalTurns;

/** 运行记录首页（最后一页，desc）。 */
export async function taskRunResume(limit: number): Promise<ThreadResumeResponse> {
  return threadResume({
    threadId: TASK_RUN_THREAD_ID,
    excludeTurns: true,
    initialTurnsPage: { limit, sortDirection: 'desc', itemsView: 'full' },
  });
}

/** 运行记录更早一页。 */
export async function taskRunTurns(params: {
  cursor: string | null;
  limit: number;
}): Promise<TurnsListResponse> {
  return threadTurnsList({
    threadId: TASK_RUN_THREAD_ID,
    cursor: params.cursor,
    limit: params.limit,
    sortDirection: 'desc',
    itemsView: 'full',
  });
}

export async function listAbilities() {
  return transmit(abilities);
}

/** 三档内置流程（只读模板；可复制成自己的）。 */
export async function listBuiltinFlows(): Promise<BuiltinFlow[]> {
  return transmit(builtinFlows);
}

export async function listFlowIndex() {
  return transmit(flowIndex);
}

/* ───────────────────────── 流程编辑器（界面文案叫「流程」，代码标识 flow） ───────────────────────── */

/**
 * 流程列表。includeStress 只有开发开关打开时才传 true：
 * 压测流程（60 节点）只用来测画布性能，不进正常列表。
 */
export async function listFlows(options?: { includeStress?: boolean }) {
  const list = options?.includeStress ? SEED_FLOWS.concat(makeStressFlow()) : SEED_FLOWS;
  return transmit(list);
}

export async function listFlowPalette() {
  return transmit(PALETTE_PARTS);
}

export async function listFlowTemplates() {
  return transmit(TEMPLATE_OPTIONS);
}

export async function listNewFlowDrafts() {
  return transmit(NEW_FLOW_DRAFTS);
}

/* 等待一帧：界面里量「画完」的时候用。 */
export function afterPaint(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}
