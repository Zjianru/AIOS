/** 档位的中文名（选择器和提示共用；纯函数，不依赖 store/组件，避免循环 import）。 */
export function effortName(effort: string): string {
  const names: Record<string, string> = {
    none: '不思考',
    minimal: '最弱',
    low: '弱',
    medium: '中',
    high: '强',
    xhigh: '更强',
    max: '最强',
  };
  return names[effort] ?? effort;
}
