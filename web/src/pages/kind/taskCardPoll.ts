/**
 * T051.6：Kind 任务卡轻轮询的调度。抽成可注入定时器/可见性/拉取的纯调度，
 * 组件只负责把 deps 接上；单测用假定时器验证「不叠」。
 *
 * 现在的行为（待 T051.6 修复）：每次拉取回来都再排一个定时器、旧的定时器不清；
 * 有拉取在途时 visibilitychange 还会再发起一次。先按现状抽出来，由单测锁定问题，再改。
 */
export interface TaskCardPollDeps {
  /** 轮询间隔（毫秒）。 */
  intervalMs: number;
  /** 拉一次任务事实；组件在这里决定写不写 store。 */
  refresh: () => Promise<void>;
  /** 现在页面可见吗；默认读 document.visibilityState。 */
  isVisible?: () => boolean;
  /** 注入定时器（单测用假实现）；默认 window.setTimeout。 */
  setTimeout?: (handler: () => void, ms: number) => number;
  /** 注入清定时器；默认 window.clearTimeout。 */
  clearTimeout?: (handle: number) => void;
}

export interface TaskCardPoll {
  /** 立即拉一次（挂载时用），回来后排下一次。 */
  refreshNow: () => void;
  /** 页面变可见时调用。 */
  onVisible: () => void;
  /** 卸载：清定时器；在途的那次回来不再写、不再排。 */
  dispose: () => void;
}

export function startTaskCardPoll(deps: TaskCardPollDeps): TaskCardPoll {
  const isVisible =
    deps.isVisible ?? (() => typeof document === 'undefined' || document.visibilityState === 'visible');
  const scheduleTimeout =
    deps.setTimeout ??
    ((handler: () => void, ms: number) => window.setTimeout(handler, ms) as unknown as number);
  const clearScheduled =
    deps.clearTimeout ?? ((handle: number) => window.clearTimeout(handle));

  let disposed = false;
  let timer: number | null = null;

  const schedule = () => {
    timer = scheduleTimeout(() => {
      timer = null;
      if (isVisible()) void refresh();
      else schedule();
    }, deps.intervalMs);
  };

  const refresh = async () => {
    await deps.refresh();
    if (disposed) return;
    schedule();
  };

  return {
    refreshNow: () => {
      void refresh();
    },
    onVisible: () => {
      if (isVisible()) void refresh();
    },
    dispose: () => {
      disposed = true;
      if (timer !== null) clearScheduled(timer);
    },
  };
}
