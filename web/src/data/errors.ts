/// T009.2：页面唯一的错误对照表（spec v1.1）。
///
/// 输入是两种码：/api 的 12 个码、日志里的模型错误码；输出中文原因、下一步和动作。
/// 页面不显示原始码和状态数字；不认识的码给通用原因。

export const API_ERROR_CODES = [
  'root/unavailable',
  'question/answer-rejected',
  'keyring/key-file-missing',
  'keyring/undecryptable',
  'keyring/corrupt',
  'keyring/io',
  'connection/missing',
  'connection/invalid',
  'request/invalid-path',
  'request/invalid-json',
  'settings/unavailable',
  'agent/internal',
  'attachment/invalid',
  'attachment/not-text',
] as const;

export const MODEL_ERROR_CODES = [
  'AUTH',
  'MISSING_CREDENTIAL',
  'KEYRING',
  'QUOTA',
  'RATE_LIMIT',
  'CONTEXT_WINDOW_EXCEEDED',
  'INVALID_REQUEST',
  'SERVER',
  'TRANSPORT',
  'TIMEOUT',
  'STREAM_CLOSED',
  'MALFORMED_RESPONSE',
  'EMPTY_RESPONSE',
  'UNSUPPORTED_CONTENT',
  'UNSUPPORTED_REASONING_EFFORT',
  'ABORTED',
] as const;

const DEFAULT_CONNECTION = '模型服务';

function connectionLabel(name: string | undefined): string {
  const trimmed = name?.trim();
  return trimmed ? trimmed : DEFAULT_CONNECTION;
}

const MODEL_REASONS: Record<string, (name: string) => string> = {
  AUTH: (name) => `${name} 拒绝了这把密钥`,
  MISSING_CREDENTIAL: (name) => `Keyring 里没有 ${name} 的密钥`,
  KEYRING: () => '读不出 Keyring：钥匙文件不在原处或解不开',
  QUOTA: (name) => `${name} 账户的额度用完了`,
  RATE_LIMIT: () => '请求太频繁',
  CONTEXT_WINDOW_EXCEEDED: () => '对话太长，超出了模型一次能读的长度',
  INVALID_REQUEST: (name) => `${name} 说这次请求不合规`,
  SERVER: (name) => `${name} 那边出错了`,
  TRANSPORT: (name) => `连不上 ${name}`,
  TIMEOUT: (name) => `${name} 太久没回应`,
  STREAM_CLOSED: () => '回答传到一半断了',
  MALFORMED_RESPONSE: (name) => `${name} 回的内容读不懂`,
  EMPTY_RESPONSE: (name) => `${name} 回了空内容`,
  UNSUPPORTED_CONTENT: () => '这个模型不支持这种内容',
  UNSUPPORTED_REASONING_EFFORT: () => '这个模型不支持所选的思考强度',
  ABORTED: () => '这一轮被停下了',
};

const API_REASONS: Record<string, string> = {
  'root/unavailable': '服务还没准备好：根会话现在不可用',
  'question/answer-rejected': '这个回答现在收不下',
  'keyring/key-file-missing': '读不出 Keyring：钥匙文件不在原处或解不开',
  'keyring/undecryptable': '读不出 Keyring：钥匙文件不在原处或解不开',
  'keyring/corrupt': '读不出 Keyring：钥匙文件坏了',
  'keyring/io': '读不出 Keyring：读写钥匙文件出错',
  'connection/missing': '设置里没有这个连接',
  'connection/invalid': '这个连接的配置不完整',
  'request/invalid-path': '发给服务的内容里路径不对',
  'request/invalid-json': '页面发给服务的内容，服务没看懂',
  'settings/unavailable': '现在还不能改设置',
  'agent/internal': '内核出错了，稍后再试',
  'attachment/invalid': '附件不对（名字或内容格式有问题）',
  'attachment/not-text': '只支持文字文件，这个附件是二进制',
};

const API_ACTIONS: Record<string, 'settings' | 'retry'> = {
  'keyring/key-file-missing': 'settings',
  'keyring/undecryptable': 'settings',
  'keyring/corrupt': 'settings',
  'keyring/io': 'settings',
  'connection/missing': 'settings',
  'connection/invalid': 'settings',
  'settings/unavailable': 'settings',
  'root/unavailable': 'retry',
  'request/invalid-json': 'retry',
};

/** 模型错误码 → 中文原因；不认识的码（含 HTTP_<状态>）给通用原因。 */
export function modelErrorReason(code: string | undefined, connectionName?: string): string {
  const name = connectionLabel(connectionName);
  const known = code ? MODEL_REASONS[code] : undefined;
  if (known) return known(name);
  return `${name} 返回了意外的错误`;
}

/** 重试行原因：和失败块同一张表。 */
export function retryReason(code: string | undefined, connectionName?: string): string {
  const name = connectionLabel(connectionName);
  const known = code ? MODEL_REASONS[code] : undefined;
  if (known) return known(name);
  return '请求失败';
}

/** /api 错误码 → 中文原因；不认识的码给通用原因。 */
export function apiErrorReason(code: string | undefined): string {
  const reason = code ? API_REASONS[code] : undefined;
  return reason ?? '服务出错了，稍后再试';
}

/** /api 错误码对应的下一步动作（设置页或再试一次）。 */
export function apiErrorAction(code: string | undefined): 'settings' | 'retry' | null {
  const action = code ? API_ACTIONS[code] : undefined;
  return action ?? null;
}
