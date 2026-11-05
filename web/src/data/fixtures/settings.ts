/**
 * 设置 › 模型与连接（Keyring）的样例数据。
 *
 * - 卡片照 DSH 的模型设置页：提供方一行一行列；一次只展开一张编辑卡片；
 *   「添加模型提供商」分两种方式：第三方模型提供商（内置目录）、自定义模型 API。
 * - 密钥正文从不出现在这里，也不出现在任何返回值里：界面只拿「有没有配、可不可用」。
 *   没有环境变量后备（D34），密钥只走 Keyring。
 * - 「已保存」和「连接可用」是两件事，分开记（docs 10 篇第 2 节）。
 */

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
  /** T012.4：美元/百万 token（用户价；不乘倍率）。 */
  price?: number;
}

export type Protocol = 'anthropic-messages' | 'openai-chat' | 'openai-responses';

export const PROTOCOL_LABEL: Record<Protocol, string> = {
  'anthropic-messages': 'Anthropic Messages',
  'openai-chat': 'OpenAI Chat Completions',
  'openai-responses': 'OpenAI Responses',
};

/** 地址占位随协议变：Anthropic 的 SDK 会自己追加 /v1/messages。 */
export const PROTOCOL_ENDPOINT_HINT: Record<Protocol, string> = {
  'anthropic-messages': 'https://gateway.example',
  'openai-chat': 'https://gateway.example/v1',
  'openai-responses': 'https://gateway.example/v1',
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
  /** 模型目录；空表示用适配器默认的模型。 */
  models: ModelRow[];
  /** T012.4：价格倍率（1 = 原价，0 = 免费）。 */
  priceRate?: number;
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

export interface ModelSettingsView {
  connections: ConnectionView[];
  catalog: CatalogProvider[];
  /** problem：钥匙文件读不出来时的原因（不在原处、打不开、解不开）；读得出来就没有这个字段。 */
  keyFile: { path: string; since: string; problem?: string };
  issuedToday: number;
  issues: IssueRecord[];
}

const DEEPSEEK_MODELS: ModelRow[] = [
  { id: 'deepseek-chat', name: 'DeepSeek Chat', contextWindow: 128000, maxTokens: 8000 },
  { id: 'deepseek-reasoner', name: 'DeepSeek Reasoner', contextWindow: 128000, maxTokens: 64000 },
];

export const modelSettingsFixture: ModelSettingsView = {
  connections: [
    {
      id: 'deepseek',
      name: 'DeepSeek',
      source: 'catalog',
      protocol: 'anthropic-messages',
      endpoint: '',
      defaultEndpoint: 'https://api.deepseek.com/anthropic',
      models: [],
      key: { saved: true, savedAt: '10月31日 21:08' },
      check: { kind: 'ok', at: '今天 11:42' },
    },
  ],
  catalog: [
    {
      id: 'deepseek',
      name: 'DeepSeek',
      protocol: 'anthropic-messages',
      defaultEndpoint: 'https://api.deepseek.com/anthropic',
      defaultModels: DEEPSEEK_MODELS,
    },
    {
      id: 'kimi',
      name: 'Kimi',
      protocol: 'openai-chat',
      defaultEndpoint: 'https://api.moonshot.cn/v1',
      defaultModels: [{ id: 'kimi-k2', name: 'Kimi K2', contextWindow: 256000 }],
    },
    {
      id: 'minimax',
      name: 'MiniMax',
      protocol: 'anthropic-messages',
      defaultEndpoint: 'https://api.minimaxi.com/anthropic',
      defaultModels: [{ id: 'MiniMax-M2', name: 'MiniMax M2', contextWindow: 200000 }],
    },
    {
      id: 'openai',
      name: 'OpenAI',
      protocol: 'openai-responses',
      defaultEndpoint: 'https://api.openai.com/v1',
      defaultModels: [{ id: 'gpt-5', name: 'GPT-5', contextWindow: 400000, maxTokens: 128000 }],
    },
    {
      id: 'anthropic',
      name: 'Anthropic',
      protocol: 'anthropic-messages',
      defaultEndpoint: 'https://api.anthropic.com',
      defaultModels: [{ id: 'claude-sonnet-5-5', name: 'Claude Sonnet 5.5', contextWindow: 1000000 }],
    },
  ],
  keyFile: { path: '~/.config/aios/keyring.key', since: '10月31日' },
  issuedToday: 132,
  issues: [
    { id: 'i6', at: '14:02', connection: 'DeepSeek', purpose: 'Kind 对话 · 模型请求', until: '14:07' },
    { id: 'i5', at: '13:58', connection: 'DeepSeek', purpose: 'Kind 对话 · 模型请求', until: '14:03' },
    { id: 'i4', at: '13:58', connection: 'DeepSeek', purpose: 'Kind 对话 · 压缩摘要', until: '14:03' },
    { id: 'i3', at: '13:41', connection: 'DeepSeek', purpose: 'Kind 对话 · 模型请求', until: '13:46' },
    { id: 'i2', at: '11:42', connection: 'DeepSeek', purpose: '设置 · 验证连接', until: '11:43' },
  ],
};

/** 「获取可用模型」时服务商回的候选（假的；真的由内核去问那个地址）。 */
export const DISCOVERED_MODELS: Record<string, ModelRow[]> = {
  deepseek: DEEPSEEK_MODELS,
  kimi: [
    { id: 'kimi-k2', name: 'Kimi K2', contextWindow: 256000 },
    { id: 'kimi-k2-turbo', name: 'Kimi K2 Turbo', contextWindow: 256000 },
  ],
  minimax: [{ id: 'MiniMax-M2', name: 'MiniMax M2', contextWindow: 200000 }],
  openai: [
    { id: 'gpt-5', name: 'GPT-5', contextWindow: 400000, maxTokens: 128000 },
    { id: 'gpt-5-mini', name: 'GPT-5 mini', contextWindow: 400000, maxTokens: 128000 },
  ],
  anthropic: [
    { id: 'claude-sonnet-5-5', name: 'Claude Sonnet 5.5', contextWindow: 1000000 },
    { id: 'claude-haiku-5-5', name: 'Claude Haiku 5.5', contextWindow: 200000 },
  ],
  custom: [
    { id: 'qwen3-coder', contextWindow: 256000 },
    { id: 'glm-4.6', contextWindow: 200000 },
    { id: 'deepseek-chat', contextWindow: 128000 },
  ],
};
