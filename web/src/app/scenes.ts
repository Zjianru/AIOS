/**
 * 场景定义（layout-motion-v1.md §一）：侧栏每个入口是一个场景，每个场景一个 dock 根、一套默认布局。
 * 最小宽度取自 §二：对话 420、在做的事 280、查看器 360、日历 480、等你 300。
 */

export interface PaneDef {
  id: string;
  title: string;
  /** 分栏的最小宽度（§二）；窄窗收起时按这个加总判断。 */
  minWidth: number;
  /** 默认布局里的初始宽度；不给就用 minWidth。 */
  defaultWidth?: number;
}

export interface SceneDef {
  key: string;
  panes: PaneDef[];
}

export const SCENES: Record<string, SceneDef> = {
  kind: {
    key: 'kind',
    panes: [
      { id: 'conversation', title: '对话', minWidth: 420 },
      { id: 'rail', title: '聚焦', minWidth: 280, defaultWidth: 380 },
    ],
  },
  ability: { key: 'ability', panes: [{ id: 'main', title: '能力', minWidth: 520 }] },
  now: { key: 'now', panes: [{ id: 'main', title: '现在', minWidth: 520 }] },
  work: {
    key: 'work',
    panes: [
      { id: 'board', title: '看板', minWidth: 520, defaultWidth: 760 },
      { id: 'waiting', title: '等你', minWidth: 300, defaultWidth: 320 },
    ],
  },
  project: {
    key: 'project',
    panes: [
      { id: 'main', title: '项目', minWidth: 520, defaultWidth: 760 },
      { id: 'chat', title: '对话', minWidth: 300, defaultWidth: 340 },
    ],
  },
  task: { key: 'task', panes: [{ id: 'main', title: '任务', minWidth: 520 }] },
  trace: { key: 'trace', panes: [{ id: 'main', title: '原始记录', minWidth: 520 }] },
  team: { key: 'team', panes: [{ id: 'main', title: '团队', minWidth: 520 }] },
  lib: { key: 'lib', panes: [{ id: 'main', title: '资料', minWidth: 520 }] },
  read: {
    key: 'read',
    panes: [
      { id: 'main', title: '阅读', minWidth: 520, defaultWidth: 760 },
      { id: 'side', title: '伴读', minWidth: 300, defaultWidth: 340 },
    ],
  },
  memory: { key: 'memory', panes: [{ id: 'main', title: '记忆', minWidth: 520 }] },
  health: { key: 'health', panes: [{ id: 'main', title: 'AIOS 健康状况', minWidth: 520 }] },
  notify: { key: 'notify', panes: [{ id: 'main', title: '通知', minWidth: 520 }] },
  usage: { key: 'usage', panes: [{ id: 'main', title: '用量与费用', minWidth: 520 }] },
  settings: { key: 'settings', panes: [{ id: 'main', title: '设置', minWidth: 520 }] },
  newproj: { key: 'newproj', panes: [{ id: 'main', title: '新建项目', minWidth: 520 }] },
  welcome: { key: 'welcome', panes: [{ id: 'main', title: '欢迎引导', minWidth: 520 }] },
  empty: { key: 'empty', panes: [{ id: 'main', title: '空状态预览', minWidth: 520 }] },
};

export const DEFAULT_SCENE = SCENES.now;

export function sceneDef(key: string): SceneDef {
  return SCENES[key] ?? DEFAULT_SCENE;
}

/** 每个路由属于第几期（ROUTES.md 的同一张表，占位页照它写）。 */
export const PERIOD_OF: Record<string, 1 | 2 | 3> = {
  kind: 1,
  ability: 1,
  flow: 1,
  now: 2,
  work: 2,
  project: 2,
  task: 2,
  trace: 2,
  read: 2,
  empty: 2,
  team: 3,
  role: 3,
  lib: 3,
  memory: 3,
  health: 3,
  notify: 3,
  usage: 3,
  settings: 3,
  monitor: 3,
  projset: 3,
  newproj: 3,
  welcome: 3,
};
