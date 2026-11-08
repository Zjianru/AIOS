/**
 * Keyring 时间的显示：内核给的是 epoch 秒（savedAt / check.at / keyFile.since / 签发记录），
 * 页面上要写成可读的本地时间（例如 2026-11-08 03:12），不出现裸时间戳。
 *
 * 演示数据里已经是可读文本（例如「今天 11:42」），原样返回，不再套一层。
 */
export function formatLocalDateTime(value: string | number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  if (typeof value === 'string' && value.trim() === '') return '—';
  const seconds = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(seconds)) return String(value);
  if (seconds <= 0) return '—';
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return '—';
  const pad = (part: number) => String(part).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
