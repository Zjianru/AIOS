/**
 * 选择器的档位事实：`session/model` 事件只带 wanted，实际档只能来自服务
 * （card-act 回执、status、session-model-select 响应）。事件处理绝不能拿 wanted 顶替 actual。
 */

export interface SelectionEffort {
  wanted?: string;
  actual?: string;
}

/** 日志事件带来的选择：只有 wanted。 */
export function eventSelectionEffort(wanted?: string): SelectionEffort {
  return { wanted, actual: undefined };
}

/** 服务给的当前选择：actual 有就用，没有就是未知。 */
export function serviceSelectionEffort(wanted?: string, actual?: string): SelectionEffort {
  return { wanted, actual };
}
