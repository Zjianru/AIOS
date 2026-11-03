/** T008.4：把服务协议 v3 的 Keyring 视图映射成设置/首次使用页用的原型类型。 */
import {
  deleteConnection,
  fetchModelSettings,
  relocateKeyring,
  reopenKeyring,
  restartKeyring,
  saveConnection,
  verifyConnection,
  type ServiceConnectionInfo,
  type ServiceKeyringView,
} from '@/data/service';
import type {
  CatalogProvider,
  CheckState,
  ConnectionView,
  ModelSettingsView,
} from '@/data/fixtures/settings';

/** 原型文案：只显示接上内核的提供方（D36：目录里只有 DeepSeek）。 */
export const KERNEL_PROVIDERS: ReadonlySet<string> = new Set(['deepseek']);

const DEEPSEEK_MODELS = [
  { id: 'deepseek-flash', name: 'DeepSeek Flash', contextWindow: 1_000_000 },
  { id: 'deepseek-pro', name: 'DeepSeek Pro', contextWindow: 1_000_000 },
];

const CATALOG: CatalogProvider[] = [
  {
    id: 'deepseek',
    name: 'DeepSeek',
    protocol: 'anthropic-messages',
    defaultEndpoint: 'https://api.deepseek.com/anthropic',
    defaultModels: DEEPSEEK_MODELS,
  },
];

function mapCheck(info: ServiceConnectionInfo): CheckState {
  if (info.check.kind === 'never') return { kind: 'never' };
  if (info.check.kind === 'ok') return { kind: 'ok', at: String(info.check.at) };
  return { kind: 'fail', at: String(info.check.at), reason: info.check.reason };
}

function mapConnection(info: ServiceConnectionInfo): ConnectionView {
  return {
    id: info.id,
    name: info.name,
    source: 'catalog',
    protocol: 'anthropic-messages',
    endpoint: info.endpoint,
    defaultEndpoint: CATALOG[0]?.defaultEndpoint ?? 'https://api.deepseek.com/anthropic',
    models: [],
    key: { saved: true, savedAt: String(info.saved_at) },
    check: mapCheck(info),
  };
}

function mapView(view: ServiceKeyringView): ModelSettingsView {
  return {
    connections: view.connections.map(mapConnection),
    catalog: CATALOG,
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
  return mapView(await fetchModelSettings());
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
  const model = draft.models[0]?.id || 'deepseek-flash';
  const endpoint = draft.endpoint || CATALOG[0]?.defaultEndpoint || '';
  await saveConnection({ id: draft.id, name: draft.name, model, endpoint, apiKey: draft.key });
  return getModelSettings();
}

/** 验证连接；结果由服务写进注册器。 */
export async function checkConnection(connectionId: string): Promise<ModelSettingsView> {
  await verifyConnection(connectionId);
  return getModelSettings();
}

/** 设置页的编辑：有 key 就 upsert；没有 key 时服务目前没有「只改地址/模型」的方法（WIP）。 */
export async function updateConnection(
  connectionId: string,
  patch: { endpoint: string; models: { id: string; name?: string }[]; key: string },
): Promise<ModelSettingsView> {
  if (!patch.key.trim()) return getModelSettings();
  const model = patch.models[0]?.id || 'deepseek-flash';
  const endpoint = patch.endpoint.trim() || CATALOG[0]?.defaultEndpoint || '';
  await saveConnection({ id: connectionId, name: connectionId, model, endpoint, apiKey: patch.key });
  return getModelSettings();
}

export async function removeConnection(connectionId: string): Promise<ModelSettingsView> {
  await deleteConnection(connectionId);
  return getModelSettings();
}

/** 撤销 key：服务还没有「只撤 key 保留连接」的方法（WIP）；先保持现状。 */
export async function revokeKey(_connectionId: string): Promise<ModelSettingsView> {
  return getModelSettings();
}

export async function discoverModels(_target: { catalogId?: string; endpoint: string }): Promise<
  { id: string; name?: string; contextWindow?: number }[]
> {
  return DEEPSEEK_MODELS;
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
  return mapView(await restartKeyring());
}

export function setKeyringProblem(_problem: string | null): void {}

export function syncKeyringEmpty(_empty: boolean): void {}
