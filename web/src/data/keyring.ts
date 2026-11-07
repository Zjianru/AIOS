/** T008.4：把服务协议 v3 的 Keyring 视图映射成设置/首次使用页用的原型类型。 */
import {
  deleteConnection,
  discoverAvailableModels,
  fetchModelSettings,
  revokeConnectionKey,
  updateConnectionInfo,
  relocateKeyring,
  reopenKeyring,
  restartKeyring,
  saveConnection,
  setDisabledModels as setDisabledModelsOnService,
  verifyConnection,
  type ServiceConnectionInfo,
  type ServiceKeyringView,
  type ServiceModelRow,
} from '@/data/service';
import type {
  CatalogProvider,
  CheckState,
  ConnectionView,
  ModelSettingsView,
} from '@/data/fixtures/settings';

/** T012.9：接上内核的提供方（D36 的 DeepSeek + T012 的 MiniMax 两家国内/国际站）。 */
export const KERNEL_PROVIDERS: ReadonlySet<string> = new Set(['deepseek', 'minimax', 'minimax-cn']);

/** T012.9：与服务内置目录一致的模型名（菜单/添加连接先列这些；真名单以「获取可用模型」为准）。 */
const DEEPSEEK_MODELS = [
  { id: 'deepseek-flash', name: 'DeepSeek-V4.1-Flash', contextWindow: 1_000_000 },
  { id: 'deepseek-v4-flash', name: 'DeepSeek V4 Flash', contextWindow: 1_000_000 },
  { id: 'deepseek-v4-flash-vision-exp', name: 'DeepSeek V4 Flash Vision Exp', contextWindow: 1_000_000 },
  { id: 'deepseek-v4-pro', name: 'DeepSeek-V4-Pro', contextWindow: 1_000_000 },
];

const MINIMAX_MODELS = [
  { id: 'MiniMax-M3.1-Flash', name: 'MiniMax-M3.1-Flash', contextWindow: 1_000_000 },
  { id: 'MiniMax-M3', name: 'MiniMax-M3', contextWindow: 1_000_000 },
  { id: 'MiniMax-M2.7', name: 'MiniMax-M2.7', contextWindow: 204_800 },
  { id: 'MiniMax-M2.5', name: 'MiniMax-M2.5', contextWindow: 204_800 },
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
  name?: string;
  contextWindow?: number;
  maxTokens?: number;
  priceInput?: number;
  priceOutput?: number;
  priceCacheRead?: number;
  priceCacheWrite?: number;
};

function mapRow(row: WireRow): WireRow {
  return {
    id: row.id,
    name: row.name,
    contextWindow: row.contextWindow,
    maxTokens: row.maxTokens,
    priceInput: row.priceInput,
    priceOutput: row.priceOutput,
    priceCacheRead: row.priceCacheRead,
    priceCacheWrite: row.priceCacheWrite,
  };
}

function mapConnection(info: ServiceConnectionInfo, listed?: WireRow[]): ConnectionView {
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
    models,
    // 跟着厂商走（models 为空）时，可选的是存下的厂商列表；没存过用内置目录。
    availableModels: listed ?? preset?.defaultModels ?? [],
    priceRate: info.price_rate,
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
  const mappedListed: Record<string, { fetchedAt: string; models: WireRow[] }> = {};
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
      problem: view.key_file.problem,
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

/** 验证连接；结果由服务写进注册器。 */
export async function checkConnection(connectionId: string): Promise<ModelSettingsView> {
  await verifyConnection(connectionId);
  return getModelSettings();
}

/** 设置页的编辑：地址/模型行/倍率一起保存；填了 key 就连 key 一起 upsert。 */
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
    priceRate?: number;
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
      price_rate: patch.priceRate,
    });
  } else {
    await updateConnectionInfo(connectionId, { endpoint, model, models: rows, price_rate: patch.priceRate });
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
    contextWindow: row.contextWindow,
    fresh: row.fresh,
    priceInput: row.priceInput,
    priceOutput: row.priceOutput,
    priceCacheRead: row.priceCacheRead,
    priceCacheWrite: row.priceCacheWrite,
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
