/**
 * 能力库的样例数据，照 v13「能力」页抄过来（app-v14-brief.md §二）。
 * 第 1 期只用到「能力 → 流程」这一组，其余条目按 v13 的样子列出来，点开是第 3 期。
 */

export type AbilityType = '工具' | '技能' | '连接' | '组合';

/** 三档内置流程（用户 2026-10-27 裁定 D-2026-10-27-2；只读、可复制成自己的）。 */
export type BuiltinFlowTier = '轻档' | '普通档' | '配对档';
export interface BuiltinFlow {
  id: string;
  name: string;
  tier: BuiltinFlowTier;
  description: string;
  steps: string[];
}

export const builtinFlows: BuiltinFlow[] = [
  {
    id: 'builtin-light',
    name: '轻档 · 直接做完',
    tier: '轻档',
    description: '小事：Kind 直接做完，把结果给你看；不设门、不中途打扰你。',
    steps: ['Kind 直接完成', '交回成果'],
  },
  {
    id: 'builtin-normal',
    name: '普通档 · 先给任务图',
    tier: '普通档',
    description: '先给你一张任务图，你确认了再开工；每站谁做、交出什么都写清。',
    steps: ['路线草稿', '你确认', '执行', '交回成果'],
  },
  {
    id: 'builtin-pair',
    name: '配对档 · 有人验',
    tier: '配对档',
    description: '两个人并行：一个人做、另一个人验，最后 Kind 核对；审查门按成员数并行。',
    steps: ['执行', '审查门', '修正', '复核', '交回成果'],
  },
];
export type AbilityStatus = 'ok' | 'needAuth' | 'needKey' | 'needDep';

export interface AbilityFixture {
  id: string;
  name: string;
  type: AbilityType;
  source: '内置' | '你装的' | '项目专属';
  equippedBy: string[];
  status: AbilityStatus;
  group: 'daily' | 'pro';
}

export const abilityGroups: { key: 'daily' | 'pro'; name: string }[] = [
  { key: 'daily', name: '日常' },
  { key: 'pro', name: '专业' },
];

export const abilities: AbilityFixture[] = [
  { id: 'web-search', name: '网页搜索', type: '工具', source: '内置', equippedBy: ['研究员', '评测员'], status: 'ok', group: 'daily' },
  { id: 'read-web', name: '读取网页', type: '工具', source: '内置', equippedBy: ['研究员'], status: 'ok', group: 'daily' },
  { id: 'write-docx', name: '写作文档（Word）', type: '技能', source: '内置', equippedBy: ['研究员'], status: 'ok', group: 'daily' },
  { id: 'daily-brief', name: '每日简报', type: '组合', source: '你装的', equippedBy: ['Kind'], status: 'ok', group: 'daily' },
  { id: 'calendar-wake', name: '日历唤醒', type: '工具', source: '内置', equippedBy: ['Kind'], status: 'ok', group: 'daily' },
  { id: 'run-command', name: '命令执行', type: '工具', source: '内置', equippedBy: ['评测员'], status: 'ok', group: 'pro' },
  { id: 'code-review', name: '代码审查', type: '技能', source: '内置', equippedBy: ['审查员'], status: 'ok', group: 'pro' },
  { id: 'github', name: 'GitHub 连接', type: '连接', source: '你装的', equippedBy: ['源码解读者'], status: 'needAuth', group: 'pro' },
  { id: 'bing-search', name: 'Bing 搜索', type: '连接', source: '你装的', equippedBy: ['评测员'], status: 'needKey', group: 'pro' },
  { id: 'dedupe-script', name: '数据去重脚本', type: '组合', source: '项目专属', equippedBy: ['评测员'], status: 'needDep', group: 'pro' },
];

/**
 * 「能力 → 流程」这一组的索引。界面文案一律叫「流程」（用户 2026-10-27 裁定），代码标识用 flow。
 * 图形数据在 src/data/flows.ts（编辑器用），这里只放列表要显示的字段。
 */
export interface FlowIndexEntry {
  id: string;
  name: string;
  version: string;
  source: string;
  usedTimes: number;
  projectCode?: string;
  projectName?: string;
}

export const flowIndex: FlowIndexEntry[] = [
  { id: 'content-ops', name: '内容运营', version: 'v3', source: '你自建', usedTimes: 12 },
  {
    id: 'aios-product',
    name: 'AIOS 产品',
    version: 'v2',
    source: '导入自 unFIX 社区模板，未带权限',
    usedTimes: 3,
  },
];
