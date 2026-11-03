/**
 * 能力库的样例数据，照 v13「能力」页抄过来（app-v14-brief.md §二）。
 * 第 1 期只用到「能力 → 流程」这一组，其余条目按 v13 的样子列出来，点开是第 3 期。
 */
export const builtinFlows = [
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
export const abilityGroups = [
    { key: 'daily', name: '日常' },
    { key: 'pro', name: '专业' },
];
export const abilities = [
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
export const flowIndex = [
    { id: 'content-ops', name: '内容运营', version: 'v3', source: '你自建', usedTimes: 12 },
    {
        id: 'aios-product',
        name: 'AIOS 产品',
        version: 'v2',
        source: '导入自 unFIX 社区模板，未带权限',
        usedTimes: 3,
    },
];
