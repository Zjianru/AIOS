/**
 * T051.6：Kind 任务卡轻轮询的调度。抽成可注入定时器/可见性/拉取的纯调度，
 * 组件只负责把 deps 接上；单测用假定时器验证「不叠」。
 *
 * 规则：排下一次之前先清掉旧定时器；有拉取在途时 visibilitychange 不再发起第二次；
 * 卸载后在途的那次回来不写、也不再排。
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
  /** 页面变可见时调用；有拉取在途时这一下会被忽略。 */
  onVisible: () => void;
  /** 卸载：清定时器；在途的那次回来不再写、不再排。 */
  dispose: () => void;
}

export function startTaskCardPoll(deps: TaskCardPollDeps): TaskCardPoll {
  const isVisible =
    deps.isVisible ??
    (() => typeof document === 'undefined' || document.visibilityState === 'visible');
  const scheduleTimeout =
    deps.setTimeout ??
    ((handler: () => void, ms: number) => window.setTimeout(handler, ms) as unknown as number);
  const clearScheduled =
    deps.clearTimeout ?? ((handle: number) => window.clearTimeout(handle));

  let disposed = false;
  let timer: number | null = null;
  let inFlight = false;

  const schedule = () => {
    if (disposed) return;
    // 排下一次之前先清旧定时器：定时器不会叠。
    if (timer !== null) {
      clearScheduled(timer);
      timer = null;
    }
    timer = scheduleTimeout(() => {
      timer = null;
      if (isVisible()) void refresh();
      else schedule();
    }, deps.intervalMs);
  };

  const refresh = async () => {
    // 有拉取在途：visibilitychange/定时器都不能再发起第二次。
    if (disposed || inFlight) return;
    inFlight = true;
    try {
      await deps.refresh();
    } finally {
      inFlight = false;
    }
    // 卸载后在途回来：不写、不再排。
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
      if (timer !== null) {
        clearScheduled(timer);
        timer = null;
      }
    },
  };
}
