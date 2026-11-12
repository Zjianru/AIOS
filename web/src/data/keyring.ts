/** T008.4：把服务协议 v3 的 Keyring 视图映射成设置/首次使用页用的原型类型。 */
import {
  checkExaKey,
  deleteConnection,
  deleteExaKey,
  discoverAvailableModels,
  fetchExaKey,
  fetchModelSettings,
  revokeConnectionKey,
  saveExaKey as saveExaKeyOnService,
  updateConnectionInfo,
  relocateKeyring,
  reopenKeyring,
  restartKeyring,
  saveConnection,
  setDisabledModels as setDisabledModelsOnService,
  verifyConnection,
  type ConnectionInfo as ServiceConnectionInfo,
  type ConnectionVerifyResult,
  type KeyringView as ServiceKeyringView,
  type ModelRow as ServiceModelRow,
  type ToolKeyView as ServiceToolKeyView,
} from '@/data/service';
/* ───────────── T017.1：设置/首次使用页的视图类型（原 fixtures/settings 的真类型） ───────────── */

export type CheckState =
  | { kind: 'never' }
  | { kind: 'checking' }
  | { kind: 'ok'; at: string }
  | { kind: 'fail'; at: string; reason: string };

export interface ModelRow {
  id: string;
  name?: string;
  /** 上下文窗口，token。没填就是继承适配器的值。 */
  contextWindow?: number;
  maxTokens?: number;
  /** 获取面板里厂商这次新给的模型（只在候选行上有；服务端算）。 */
  fresh?: boolean;
  /** T012 F1：用户填的四类单价（美元/百万 token；空=继承目录标价）。 */
  priceInput?: number;
  priceOutput?: number;
  priceCacheRead?: number;
  priceCacheWrite?: number;
}

export type Protocol = 'anthropic-messages' | 'openai-chat' | 'openai-responses';

export const PROTOCOL_LABEL: Record<Protocol, string> = {
  'anthropic-messages': 'Anthropic Messages',
  'openai-chat': 'OpenAI Chat Completions',
  'openai-responses': 'OpenAI Responses',
};

export interface ConnectionView {
  id: string;
  name: string;
  /** catalog：来自内置目录；custom：用户自己声明的接口。 */
  source: 'catalog' | 'custom';
  protocol: Protocol;
  /** 用户改过的地址；空就是用默认地址。 */
  endpoint: string;
  defaultEndpoint: string;
  /** 连接当前用的模型（服务端存的旧字段 model）。验证弹窗默认选它。 */
  model: string;
  /** 模型目录；空表示跟着厂商的列表走（可选 = availableModels）。 */
  models: ModelRow[];
  /** 跟着厂商走时选择器/小标签可选的模型（存下的厂商列表；没存过用内置目录）。 */
  availableModels?: ModelRow[];
  key: { saved: false } | { saved: true; savedAt: string };
  check: CheckState;
}

/** 内置目录里的提供方（「第三方模型提供商」的下拉）。 */
export interface CatalogProvider {
  id: string;
  name: string;
  protocol: Protocol;
  defaultEndpoint: string;
  defaultModels: ModelRow[];
}

export interface IssueRecord {
  id: string;
  at: string;
  connection: string;
  purpose: string;
  until: string;
}

/** 设置页「联网搜索」：Exa 凭证的存没存、验证结果（没有 key 正文）。 */
export interface ToolKeyView {
  key: { saved: false } | { saved: true; savedAt: string };
  check: CheckState;
}

export interface ModelSettingsView {
  connections: ConnectionView[];
  catalog: CatalogProvider[];
  /** 服务端存下的厂商列表（按连接 id；页面小标签用它）。 */
  listed?: Record<string, { fetchedAt: string; models: ModelRow[] }>;
  /** 屏蔽的模型规则（通配；服务端保存的整表）。 */
  disabledModels: string[];
  /** 与 disabledModels 等长同序：每条规则现在命中的「连接/模型」（服务端算，页面不自己匹配）。 */
  disabledHits: string[][];
  /** settings.json 读不出来、而且这个进程从没读到过：不知道哪些被屏蔽。 */
  disabledUnknown: boolean;
  /** problem：钥匙文件读不出来时的原因（不在原处、打不开、解不开）；读得出来就没有这个字段。 */
  keyFile: { path: string; since: string; problem?: string };
  issuedToday: number;
  issues: IssueRecord[];
}

/** T012.9：接上内核的提供方（D36 的 DeepSeek + T012 的 MiniMax 两家国内/国际站）。 */
export const KERNEL_PROVIDERS: ReadonlySet<string> = new Set(['deepseek', 'minimax', 'minimax-cn']);

/** T012.9：与服务内置目录一致的模型名（菜单/添加连接先列这些；真名单以「获取可用模型」为准）。 */
const DEEPSEEK_MODELS = [
  { id: 'deepseek-flash', name: 'DeepSeek-V4.1-Flash', contextWindow: 1_000_000 },
  { id: 'deepseek-v4-flash', name: 'DeepSeek V4 Flash', contextWindow: 1_000_000 },
  { id: 'deepseek-v4-flash-vision-exp', name: 'DeepSeek V4 Flash Vision Exp', contextWindow: 1_000_000 },
  { id: 'deepseek-v4-pro', name: 'DeepSeek-V4-Pro', contextWindow: 1_000_000 },
];

// 和内核内置目录一致（8 条）：没 fetch 过时菜单/小标签只列这些，并原样沿用目录顺序。
// MiniMax-M2.1-highspeed 是厂商早先给过的旧模型，目录已不收；没 fetch 过时不预先显示，
// 厂商 live 里真给了的话，获取后照常出现。
const MINIMAX_MODELS = [
  { id: 'MiniMax-M3.1-Flash', name: 'MiniMax-M3.1-Flash', contextWindow: 1_000_000 },
  { id: 'MiniMax-M3', name: 'MiniMax-M3', contextWindow: 1_000_000 },
  { id: 'MiniMax-M2.7', name: 'MiniMax-M2.7', contextWindow: 204_800 },
  { id: 'MiniMax-M2.7-highspeed', name: 'MiniMax-M2.7-highspeed', contextWindow: 204_800 },
  { id: 'MiniMax-M2.5', name: 'MiniMax-M2.5', contextWindow: 204_800 },
  { id: 'MiniMax-M2.5-highspeed', name: 'MiniMax-M2.5-highspeed', contextWindow: 204_800 },
  { id: 'MiniMax-M2.1', name: 'MiniMax-M2.1', contextWindow: 204_800 },
  { id: 'MiniMax-M2', name: 'MiniMax-M2', contextWindow: 204_800 },
];

const CATALOG: CatalogProvider[] = [
  {
    id: 'deepseek',
    name: 'DeepSeek',
    protocol: 'anthropic-messages',
    defaultEndpoint: 'https://api.deepseek.com/anthropic',
    defaultModels: DEEPSEEK_MODELS,
  },
  {
    id: 'minimax-cn',
    name: 'MiniMax（国内站）',
    protocol: 'anthropic-messages',
    defaultEndpoint: 'https://api.minimaxi.com/anthropic',
    defaultModels: MINIMAX_MODELS,
  },
  {
    id: 'minimax',
    name: 'MiniMax（国际站）',
    protocol: 'anthropic-messages',
    defaultEndpoint: 'https://api.minimax.io/anthropic',
    defaultModels: MINIMAX_MODELS,
  },
];

function mapCheck(info: ServiceConnectionInfo): CheckState {
  if (info.check.kind === 'never') return { kind: 'never' };
  if (info.check.kind === 'ok') return { kind: 'ok', at: String(info.check.at) };
  return { kind: 'fail', at: String(info.check.at), reason: info.check.reason };
}

/** 线上模型行/目录行（都是 camelCase）映射成页面行。 */
type WireRow = {
  id: string;
  name?: string | null;
  contextWindow?: number | null;
  maxTokens?: number | null;
  priceInput?: number | null;
  priceOutput?: number | null;
  priceCacheRead?: number | null;
  priceCacheWrite?: number | null;
};

/** 线上可空字段（null）收成页面类型的可选字段（undefined）。 */
function mapRow(row: WireRow): ModelRow {
  return {
    id: row.id,
    name: row.name ?? undefined,
    contextWindow: row.contextWindow ?? undefined,
    maxTokens: row.maxTokens ?? undefined,
    priceInput: row.priceInput ?? undefined,
    priceOutput: row.priceOutput ?? undefined,
    priceCacheRead: row.priceCacheRead ?? undefined,
    priceCacheWrite: row.priceCacheWrite ?? undefined,
  };
}

function mapConnection(info: ServiceConnectionInfo, listed?: ModelRow[]): ConnectionView {
  const preset = CATALOG.find((item) => item.id === info.preset);
  const models = (info.models ?? []).map((row) => {
    const known = CATALOG.flatMap((item) => item.defaultModels).find((model) => model.id === row.id);
    return {
      ...mapRow(row),
      name: row.name ?? known?.name,
      contextWindow: row.contextWindow ?? known?.contextWindow,
    };
  });
  return {
    id: info.id,
    name: info.name,
    source: preset ? 'catalog' : 'custom',
    protocol: 'anthropic-messages',
    endpoint: info.endpoint,
    defaultEndpoint: preset?.defaultEndpoint ?? CATALOG[0]?.defaultEndpoint ?? 'https://api.deepseek.com/anthropic',
    model: info.model,
    models,
    // 跟着厂商走（models 为空）时，可选的是存下的厂商列表；没存过用内置目录。
    availableModels: listed ?? preset?.defaultModels ?? [],
    key: info.key_saved ? { saved: true, savedAt: String(info.saved_at) } : { saved: false },
    check: mapCheck(info),
  };
}

function mapView(
  view: ServiceKeyringView,
  disabledModels: string[],
  listed: Record<string, { fetchedAt: number; models: WireRow[] }> = {},
  disabledHits: string[][] = [],
  disabledUnknown = false,
): ModelSettingsView {
  const mappedListed: Record<string, { fetchedAt: string; models: ModelRow[] }> = {};
  for (const [id, entry] of Object.entries(listed)) {
    mappedListed[id] = { fetchedAt: String(entry.fetchedAt), models: entry.models.map(mapRow) };
  }
  return {
    connections: view.connections.map((info) => mapConnection(info, mappedListed[info.id]?.models)),
    catalog: CATALOG,
    listed: mappedListed,
    disabledModels,
    disabledHits,
    disabledUnknown,
    keyFile: {
      path: view.key_file.path,
      since: String(view.key_file.since),
      problem: view.key_file.problem ?? undefined,
    },
    issuedToday: view.issued_today,
    issues: view.issues.map((issue) => ({
      id: String(issue.id),
      at: String(issue.at),
      connection: issue.connection,
      purpose: issue.purpose,
      until: issue.run ?? '',
    })),
  };
}

export async function getModelSettings(): Promise<ModelSettingsView> {
  const response = await fetchModelSettings();
  return mapView(
    response.settings,
    response.disabledModels ?? [],
    response.listed ?? {},
    response.disabledHits ?? [],
    response.disabledUnknown ?? false,
  );
}

/** 整表替换屏蔽规则；返回新的设置视图（立即生效）。 */
export async function setDisabledModels(models: string[]): Promise<ModelSettingsView> {
  await setDisabledModelsOnService(models);
  // 规则表和命中表从同一次读里取：页面的「现在匹配 n 个」按位置对齐，不能分两次拼。
  const response = await fetchModelSettings();
  return mapView(
    response.settings,
    response.disabledModels ?? [],
    response.listed ?? {},
    response.disabledHits ?? [],
    response.disabledUnknown ?? false,
  );
}

export interface ConnectionDraft {
  source: 'catalog' | 'custom';
  id: string;
  name: string;
  protocol: string;
  endpoint: string;
  models: { id: string; name?: string }[];
  key: string;
}

/** 存连接；key 只在这一次请求体里进服务。 */
export async function addConnection(draft: ConnectionDraft): Promise<ModelSettingsView> {
  const preset = KERNEL_PROVIDERS.has(draft.id) ? draft.id : undefined;
  const fallback = CATALOG.find((item) => item.id === draft.id)?.defaultModels ?? CATALOG[0]?.defaultModels ?? [];
  // 一个都不勾 = 空表：跟着厂商的列表走（服务端存的 model 只作旧字段回退）。
  const rows: ServiceModelRow[] = draft.models.map((row) => ({ id: row.id, name: row.name }));
  const model = rows[0]?.id || fallback[0]?.id || 'deepseek-flash';
  const endpoint = draft.endpoint || CATALOG.find((item) => item.id === draft.id)?.defaultEndpoint || CATALOG[0]?.defaultEndpoint || '';
  await saveConnection({ id: draft.id, name: draft.name, model, endpoint, apiKey: draft.key, preset, models: rows });
  return getModelSettings();
}

/** 验证连接；不带 model 是旧路径，结果由服务写进注册器。 */
export async function checkConnection(connectionId: string): Promise<ModelSettingsView> {
  await verifyConnection(connectionId);
  return getModelSettings();
}

/** 弹窗路径：只验证选中的模型，结果只在这一次返回，不写连接状态。 */
export async function verifyConnectionForModel(
  connectionId: string,
  model: string,
): Promise<ConnectionVerifyResult> {
  const { result } = await verifyConnection(connectionId, model);
  if (!result) throw new Error('服务没有返回这次验证的结果');
  return result;
}

/** 设置页的编辑：地址/模型行一起保存；填了 key 就连 key 一起 upsert。 */
export async function updateConnection(
  connectionId: string,
  patch: {
    endpoint: string;
    models: {
      id: string;
      name?: string;
      contextWindow?: number;
      maxTokens?: number;
      priceInput?: number;
      priceOutput?: number;
      priceCacheRead?: number;
      priceCacheWrite?: number;
    }[];
    key: string;
  },
): Promise<ModelSettingsView> {
  const model = patch.models[0]?.id || 'deepseek-flash';
  const endpoint = patch.endpoint.trim() || CATALOG[0]?.defaultEndpoint || '';
  const rows: ServiceModelRow[] = patch.models
    .filter((row) => row.id.trim())
    .map((row) => ({
      id: row.id.trim(),
      name: row.name,
      contextWindow: row.contextWindow,
      maxTokens: row.maxTokens,
      priceInput: row.priceInput,
      priceOutput: row.priceOutput,
      priceCacheRead: row.priceCacheRead,
      priceCacheWrite: row.priceCacheWrite,
    }));
  if (patch.key.trim()) {
    await saveConnection({
      id: connectionId,
      name: connectionId,
      model,
      endpoint,
      apiKey: patch.key,
      models: rows,
    });
  } else {
    await updateConnectionInfo(connectionId, { endpoint, model, models: rows });
  }
  return getModelSettings();
}

export async function removeConnection(connectionId: string): Promise<ModelSettingsView> {
  await deleteConnection(connectionId);
  return getModelSettings();
}

/** 撤销 key：连接保留，之后的签发失败。 */
export async function revokeKey(connectionId: string): Promise<ModelSettingsView> {
  await revokeConnectionKey(connectionId);
  return getModelSettings();
}

/** 用户按「获取可用模型」：从服务问这个连接现在可用的模型（目录 + 厂商列表合并；带窗口/标价）。 */
export async function discoverModels(connectionId: string): Promise<
  {
    id: string;
    name?: string;
    contextWindow?: number;
    fresh?: boolean;
    priceInput?: number;
    priceOutput?: number;
    priceCacheRead?: number;
    priceCacheWrite?: number;
  }[]
> {
  const { models } = await discoverAvailableModels(connectionId);
  return models.map((row) => ({
    id: row.id,
    name: row.name,
    contextWindow: row.contextWindow ?? undefined,
    fresh: row.fresh,
    priceInput: row.priceInput ?? undefined,
    priceOutput: row.priceOutput ?? undefined,
    priceCacheRead: row.priceCacheRead ?? undefined,
    priceCacheWrite: row.priceCacheWrite ?? undefined,
  }));
}

export async function moveKeyFile(path: string): Promise<ModelSettingsView> {
  const clean = path.trim();
  if (!clean.startsWith('/') && !clean.startsWith('~/')) throw new Error('要写完整路径，以 / 或 ~/ 开头');
  await relocateKeyring(clean);
  return getModelSettings();
}

export async function relinkKeyFile(path: string): Promise<ModelSettingsView> {
  const clean = path.trim();
  if (!clean.startsWith('/') && !clean.startsWith('~/')) throw new Error('要写完整路径，以 / 或 ~/ 开头');
  await reopenKeyring(clean);
  return getModelSettings();
}

export async function resetKeyring(): Promise<ModelSettingsView> {
  await restartKeyring();
  // 重启只换钥匙文件；屏蔽表还在 settings.json 里，跟着重读一次视图。
  return getModelSettings();
}

/* ───────────────────────── 设置页「联网搜索」（D73） ───────────────────────── */

function mapToolKey(view: ServiceToolKeyView): ToolKeyView {
  const key: ToolKeyView['key'] = view.key.saved
    ? { saved: true, savedAt: String(view.key.savedAt ?? 0) }
    : { saved: false };
  let check: CheckState;
  switch (view.check.kind) {
    case 'never':
      check = { kind: 'never' };
      break;
    case 'ok':
      check = { kind: 'ok', at: String(view.check.at) };
      break;
    case 'fail':
      check = { kind: 'fail', at: String(view.check.at), reason: view.check.reason };
      break;
  }
  return { key, check };
}

export async function getExaKey(): Promise<ToolKeyView> {
  return mapToolKey(await fetchExaKey());
}

export async function saveExaKey(key: string): Promise<ToolKeyView> {
  return mapToolKey(await saveExaKeyOnService(key));
}

export async function verifyExaKey(): Promise<ToolKeyView> {
  return mapToolKey(await checkExaKey());
}

export async function removeExaKey(): Promise<ToolKeyView> {
  return mapToolKey(await deleteExaKey());
}
