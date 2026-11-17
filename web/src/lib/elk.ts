/**
 * elk 懒加载的唯一入口：能力页流程图和任务图共用。
 *
 * elkjs 打包出来约 1.5MB，只有打开需要排版的页面才用得到；动态 import 让它单独成块。
 */
export type ElkEngine = InstanceType<(typeof import('elkjs/lib/elk.bundled.js'))['default']>;

let elkPromise: Promise<ElkEngine> | null = null;

export async function getElk(): Promise<ElkEngine> {
  if (!elkPromise) {
    elkPromise = import('elkjs/lib/elk.bundled.js').then((module) => new module.default());
  }
  return elkPromise;
}
