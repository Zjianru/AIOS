/** Unix 秒 → 本地日期时间（YYYY-MM-DD HH:mm）；演示数据里已是可读文本的原样返回。 */
export function formatLocalDateTime(value: string | number | null | undefined): string {
  return String(value ?? '');
}
