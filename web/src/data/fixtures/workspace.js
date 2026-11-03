/**
 * v13 里已有的样例数据，照抄过来（需求 §二「v13 里的三个项目、全部任务、角色、资料、成果，从 v13 的数据抄过来」）。
 * 第 1 期只用到项目与任务（Kind 右栏「在做的事」的地铁线小图），其余留给第 2、3 期的页面。
 * 数据形状保持 v13 的样子，不在这里改造；页面要用的派生计算放到 selectors.ts。
 */
export const projects = {
    wsp: {
        id: 'wsp',
        name: 'web-search-pro 研究',
        code: 'W',
        color: 'var(--line-w)',
        brief: '读清 Zjianru/web-search-pro 的实现与契约，探索新场景，与同类方案实证对比，最后回答：这一类能力今天还有没有价值。',
        milestones: [
            {
                id: 'm1',
                name: '弄清现状',
                state: 'passed',
                when: '10月22日 你确认通过',
                criteria: [
                    ['读清实现与对外契约', 'ok'],
                    ['跑通无 key 基线', 'ok'],
                    ['列出已知限制', 'ok'],
                ],
            },
            {
                id: 'm2',
                name: '给出判断',
                state: 'open',
                due: '目标 10月28日',
                criteria: [
                    ['覆盖 3 类以上替代方案的对比矩阵', 'ok'],
                    ['至少 3 个引擎的实测证据', 'ok'],
                    ['给出继续 / 改造 / 放弃的判断与理由', 'todo'],
                ],
            },
        ],
    },
    news: {
        id: 'news',
        name: 'news-intelligence',
        code: 'N',
        color: 'var(--line-n)',
        brief: '读取并弄清私有仓库 news-intelligence：它是什么、代码结构与技术栈、如何运行、当前完成度与可改进点。',
        milestones: [
            {
                id: 'n1',
                name: '读懂仓库',
                state: 'judging',
                due: '目标 10月25日',
                criteria: [
                    ['说明技术栈与模块划分', 'ok', '结构说明.docx 第 1–2 节'],
                    ['写清楚怎么运行', 'ok', '结构说明.docx 第 3 节，已实际跑通'],
                    ['评估完成度', 'ok', '结构说明.docx 第 4 节'],
                    ['列出可改进点并排序', 'no', '只列了 4 条，没有排序'],
                ],
            },
        ],
    },
    daily: {
        id: 'daily',
        name: '日常',
        code: '日',
        color: 'var(--line-d)',
        brief: '定时与零散事务。',
        milestones: [{ id: 'd1', name: '每天', state: 'open', due: '每日 9:00' }],
    },
};
const M = 60;
export const tasks = {
    read: { id: 'read', project: 'wsp', ms: 'm1', title: '读清实现与契约', children: ['read1', 'read2'] },
    read1: { id: 'read1', project: 'wsp', title: '代码结构', role: '研究员', model: 'DeepSeek V4 Pro', state: 'done', elapsed: 21 * M, tokens: 12400, out: '实现说明.docx' },
    read2: { id: 'read2', project: 'wsp', title: '对外契约', role: '研究员', model: 'DeepSeek V4 Pro', state: 'done', elapsed: 12 * M, tokens: 7300, out: '契约清单.pdf', deps: ['read1'] },
    root: { id: 'root', project: 'wsp', ms: 'm2', title: '它还有没有价值，值不值得继续', children: ['A', 'B', 'C', 'D'] },
    A: { id: 'A', project: 'wsp', title: '竞争景观', role: '研究员', model: 'DeepSeek V4 Pro', state: 'done', elapsed: 18 * M, tokens: 9100, note: '3 类方案', out: '同类方案对比矩阵.xlsx' },
    B: { id: 'B', project: 'wsp', title: '实证评测', children: ['B1', 'B2'] },
    B1: { id: 'B1', project: 'wsp', title: '无 key 基线', role: '评测员', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 9 * M, tokens: 4200, out: '基线结果.json' },
    B2: { id: 'B2', project: 'wsp', title: '多引擎实测', role: '评测员', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 22 * M, tokens: 11300, note: '3 / 4 引擎', out: '评测记录.md', deps: ['B1'] },
    C: { id: 'C', project: 'wsp', title: '可能性探索', children: ['C1', 'C2', 'C3'], deps: ['A', 'B'] },
    C1: { id: 'C1', project: 'wsp', title: '场景发现', role: '研究员', model: 'DeepSeek V4 Pro', state: 'done', elapsed: 14 * M, tokens: 6800, note: '4 个场景', out: '场景草图.png' },
    C2: { id: 'C2', project: 'wsp', title: '可行性验证', role: '研究员', model: 'DeepSeek V4 Pro', state: 'run', elapsed: 9 * M, tokens: 5100, note: '验证到场景 3', deps: ['C1'], log: [
            ['12:58', '场景 1：给研究类任务做来源交叉核实 —— 可行'],
            ['13:04', '场景 2：离线环境下的缓存搜索 —— 需要验证'],
            ['13:09', '开始验证场景 3'],
        ] },
    C3: { id: 'C3', project: 'wsp', title: '要不要申请搜索 key', role: '你', state: 'wait', deps: ['C1'], note: '等你决定', ask: '第 4 个引擎（Bing）需要 key。申请后可以补测，结论更完整；不申请也能继续，结论里会注明缺这一项。' },
    D: { id: 'D', project: 'wsp', title: '价值判断', role: '待指派', state: 'todo', deps: ['C'], needsAssign: true },
    news: { id: 'news', project: 'news', ms: 'n1', title: '导入仓库并写结构说明', children: ['N1', 'N2', 'N3', 'N4'] },
    N1: { id: 'N1', project: 'news', title: '拉取仓库', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 3 * M, tokens: 1900, note: '1,284 个文件' },
    N2: { id: 'N2', project: 'news', title: '梳理结构', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 11 * M, tokens: 9800, note: '6 个模块', deps: ['N1'], out: '模块依赖图.png' },
    N3: { id: 'N3', project: 'news', title: '写说明', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 8 * M, tokens: 7700, out: '结构说明.docx', deps: ['N2'] },
    N4: { id: 'N4', project: 'news', title: '验收结构说明', role: '你', state: 'wait', deps: ['N3'], note: '41 分钟前起', ask: '结构说明写好了：技术栈、模块划分、怎么运行、完成度与可改进点。看一眼是否符合你的要求。' },
    daily: { id: 'daily', project: 'daily', ms: 'd1', title: '每日 AI 简报 · 今天', children: ['D1', 'D2', 'D3', 'D4'] },
    D1: { id: 'D1', project: 'daily', title: '搜集', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 2 * M, tokens: 2600, note: '41 条' },
    D2: { id: 'D2', project: 'daily', title: '核实来源', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'fail', elapsed: 1 * M, tokens: 1200, note: '2 个来源打不开，Kind 已用备用来源补回 2 条', deps: ['D1'],
        ask: 'The Information 与 Semafor 返回 403，重试仍失败。Kind 已改用备用来源补回 2 条，简报仍缺 1 条。是否长期更换这两个来源，已转到「现在 › 等你决定」。',
        log: [['09:01', 'theinformation.com 返回 403，重试 2 次仍失败'], ['09:01', 'semafor.com 返回 403'], ['09:02', '跳过 3 条只来自这两个来源的新闻'], ['09:06', '用备用来源补回 2 条']] },
    D3: { id: 'D3', project: 'daily', title: '成稿', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 2 * M, tokens: 3400, note: '12 条', out: '今日简报.md', deps: ['D2'] },
    D4: { id: 'D4', project: 'daily', title: '发送', role: 'Kind', model: 'MiniMax M3.1 Flash', state: 'done', elapsed: 20, tokens: 200, note: '9:06', deps: ['D3'] },
};
for (const t of Object.values(tasks)) {
    for (const c of t.children ?? [])
        tasks[c].parent = t.id;
}
/** 根任务（没有 parent，也没有 children 的那批是叶子；这里的根是项目下的聚合任务）。 */
export const rootTaskIds = ['daily', 'news', 'root'];
export const roles = [
    { name: '研究员', color: 'var(--line-w)', duty: '调研、对比、写结论；先找一手来源', tag: '常驻', abilities: ['网页搜索', '读取网页', '写作文档', '对比矩阵'], since: '已参与 7 项任务' },
    { name: '评测员', color: 'var(--line-n)', duty: '把东西真正跑起来，拿实测证据', tag: '常驻', abilities: ['命令执行', '搜索引擎实测', '数据去重脚本'], since: '已参与 4 项任务' },
    { name: '审查员', color: 'var(--line-d)', duty: '对照要求逐条检查，只认实际文件', tag: '常驻', abilities: ['读文件', '代码审查', '机器检查'], since: '已参与 5 项任务' },
    { name: '源码解读者', color: 'var(--ink-3)', duty: '逐行读透 web-search-pro 的实现与契约', tag: '项目专属 · web-search-pro', abilities: ['读仓库', '结构图'], since: '已参与 2 项任务' },
];
export const materials = [
    { name: '我的简介.md', type: 'md', status: 'ok', version: 'v4 · 10月20日', source: '你写的', placedAt: [['全局', '查']], usedBy: [['每日 AI 简报 · 今天', '读者是谁一段', 'v4']] },
    { name: '写作规范.md', type: 'md', status: 'ok', version: 'v2 · 10月18日', source: '你写的', placedAt: [['角色 · 研究员', '带'], ['项目 · 日常', '带']], usedBy: [['竞争景观', '整份', 'v2'], ['成稿', '整份', 'v2']] },
    { name: 'web-search-pro README', type: 'html', status: 'ok', version: 'v2 · 10月22日抓取', source: 'github.com/Zjianru/web-search-pro', placedAt: [['项目 · web-search-pro 研究', '查']], usedBy: [['代码结构', '§安装、§配置', 'v1'], ['对外契约', '§命令行参数', 'v2']] },
    { name: 'Brave Search API 定价', type: 'html', status: 'ok', version: 'v1 · 10月21日抓取', source: 'brave.com/search/api', placedAt: [['项目 · web-search-pro 研究', '查'], ['角色 · 评测员', '查']], usedBy: [['多引擎实测', '免费额度表', 'v1']] },
    { name: '搜索引擎评测方法.pdf', type: 'pdf', status: 'ok', version: 'v1 · 10月15日', source: '你上传的', placedAt: [['角色 · 评测员', '查'], ['角色 · 研究员', '查']], usedBy: [['无 key 基线', '§3 指标定义', 'v1']] },
    { name: '简报来源清单.xlsx', type: 'xlsx', status: 'ok', version: 'v6 · 今天 08:40', source: '你上传的', placedAt: [['项目 · 日常', '带']], usedBy: [['搜集', '整表', 'v6']] },
    { name: 'news-intelligence 仓库', type: 'sh', status: 'busy', note: '正在整理，完成 62%', version: '10月23日拉取', source: 'github.com/…/news-intelligence（私有）', placedAt: [['项目 · news-intelligence', '查']], usedBy: [['梳理结构', '6 个模块入口', '—']] },
    { name: '外包协议草案.docx', type: 'docx', status: 'ok', version: 'v1 · 10月23日', source: '你在对话里拖进来的', placedAt: [['只在这次 · Kind 对话 10月23日', '处理']], usedBy: [['Kind 对话 10月23日', '整份审读', 'v1']] },
    { name: 'Semafor 订阅页', type: 'html', status: 'bad', note: '打不开 · 返回 403', version: '10月12日抓取', source: 'semafor.com', placedAt: [['项目 · 日常', '查']], usedBy: [] },
    { name: '竞品首页截图.png', type: 'png', status: 'ok', version: 'v1 · 10月21日', source: '你上传的', placedAt: [], usedBy: [] },
];
export const artifacts = {
    '同类方案对比矩阵.xlsx': { name: '同类方案对比矩阵.xlsx', type: 'xlsx', version: 2, size: '38 KB', status: 'pend', use: '可用于价值判断' },
    '实现说明.docx': { name: '实现说明.docx', type: 'docx', version: 1, size: '212 KB', status: 'acc' },
    '契约清单.pdf': { name: '契约清单.pdf', type: 'pdf', version: 1, size: '96 KB', status: 'acc' },
    '基线结果.json': { name: '基线结果.json', type: 'json', version: 1, size: '4 KB', status: 'acc' },
    '评测记录.md': { name: '评测记录.md', type: 'md', version: 2, size: '11 KB', status: 'pend', use: '复核通过' },
    '场景草图.png': { name: '场景草图.png', type: 'png', version: 1, size: '640 KB', status: 'none', use: '仅供讨论' },
    '价值判断汇报.pptx': { name: '价值判断汇报.pptx', type: 'pptx', version: 0, size: '草稿', status: 'none', use: '草稿 · 价值判断完成后定稿' },
    '结构说明.docx': { name: '结构说明.docx', type: 'docx', version: 1, size: '184 KB', status: 'pend' },
    '模块依赖图.png': { name: '模块依赖图.png', type: 'graph', version: 1, size: '120 KB', status: 'none' },
    'run.sh': { name: 'run.sh', type: 'sh', version: 1, size: '1 KB', status: 'none', use: '已实际跑通' },
    '今日简报.md': { name: '今日简报.md', type: 'md', version: 1, size: '6 KB', status: 'none', use: '缺 3 条' },
    '来源快照 · semafor.html': { name: '来源快照 · semafor.html', type: 'html', version: 1, size: '保存于 09:01', status: 'none', use: '返回 403 时的页面' },
};
/** Kind 的工作日志（§6.3）：逐条一条，只增不改。 */
export const kindLog = [
    { at: '14:02', text: '为 web-search-pro 新增第 5 站「价值判断」', project: 'wsp' },
    { at: '13:20', text: '后台服务重启一次，核对当时在跑的任务', project: '' },
    { at: '13:09', text: '可行性验证进入场景 3', project: 'wsp' },
    { at: '12:41', text: '多引擎实测复核通过，评测记录.md 升到 v2', project: 'wsp' },
    { at: '11:38', text: '结构说明.docx 写完，等你验收', project: 'news' },
    { at: '10:12', text: '记下「简报不超过 12 条」', project: 'daily' },
    { at: '09:06', text: '每日 AI 简报已发送，缺 3 条', project: 'daily' },
    { at: '09:02', text: '两个来源返回 403，改用备用来源', project: 'daily' },
];
