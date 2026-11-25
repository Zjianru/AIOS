/**
 * 用量等待的唯一文案函数：卡片和任务对话头部都调它，服务端只给绝对毫秒。
 *
 * 今天只写时刻，跨天带日期；用浏览器本地时区格式化，不在服务端拼字符串。
 */
export function usageResumeLine(
  resumeAt: number | null | undefined,
  now: number = Date.now(),
): string {
  if (resumeAt === null || resumeAt === undefined || !Number.isFinite(resumeAt)) return '';
  const at = new Date(resumeAt);
  const current = new Date(now);
  if (Number.isNaN(at.getTime()) || Number.isNaN(current.getTime())) return '';
  const clock = String(at.getHours()).padStart(2, '0') + ':' + String(at.getMinutes()).padStart(2, '0');
  const sameDay =
    at.getFullYear() === current.getFullYear() &&
    at.getMonth() === current.getMonth() &&
    at.getDate() === current.getDate();
  return sameDay
    ? '用量到上限，' + clock + ' 自动接着做'
    : '用量到上限，' + (at.getMonth() + 1) + '月' + at.getDate() + '日 ' + clock + ' 自动接着做';
}
