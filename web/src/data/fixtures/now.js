/**
 * 「现在」首页的样例（experience-spec-v1.md §3；文案照 v13 的「现在」页）。
 *
 * 分组：等你决定（暖橙）→ 出了问题（红，分 LLM 问题 / AIOS 问题，只陈述「发生了什么 → Kind 怎么处理」，
 * 不放决定按钮）→ 在推进（蓝，一个根任务一张卡，卡里一条地铁线）→ 本周完成一行。
 *
 * 派生的部分（地铁线上的站、进度）从 tasks 算，保证和其它页一致；
 * 只有「等你决定」卡片里的选项/动作是这一页独有的文案。
 */
import { tasks } from './workspace';
function station(id, extra) {
    const task = tasks[id];
    return {
        id,
        title: task.title,
        role: task.role,
        meta: task.note,
        state: task.state ?? 'todo',
        href: 'task:' + id,
        ...extra,
    };
}
export const nowFixture = {
    greeting: '下午好',
    decisions: [
        {
            id: 'bing-key',
            kind: 'askDecision',
            typeLabel: '等你决定',
            projectCode: 'W',
            projectName: 'web-search-pro 研究',
            taskTitle: '要不要申请搜索 key',
            title: '场景 4 要不要申请 Bing 的 key？',
            why: 'Bing 引擎缺 key，第 4 个引擎无法实测。申请后可以补测、结论更完整；不申请也能继续，结论里会注明缺这一项。',
            options: ['申请 key，补测 Bing', '先不申请，结论里注明缺 Bing'],
            actions: [],
            time: '13:48',
            href: 'task:C3',
            fromProblem: true,
        },
        {
            id: 'accept-news',
            kind: 'artifact',
            typeLabel: '成果',
            projectCode: 'N',
            projectName: 'news-intelligence',
            taskTitle: '导入仓库并写结构说明',
            title: '结构说明.docx v1 写好了',
            why: '技术栈、模块划分、怎么运行、完成度与可改进点。看一眼是否符合你的要求。',
            artifact: { name: '结构说明.docx', version: 'v1' },
            actions: [{ label: '验收', primary: true }, { label: '打开' }],
            time: '昨天 18:40',
            href: 'task:N4',
        },
        {
            id: 'assign-value',
            kind: 'assignRole',
            typeLabel: '指派角色',
            projectCode: 'W',
            projectName: 'web-search-pro 研究',
            taskTitle: '价值判断',
            title: '「价值判断」交给研究员？',
            why: '最后一站还没指派。研究员参与过前几站，接得上。',
            actions: [{ label: '确认', primary: true }, { label: '换人' }],
            time: '11:43',
            href: 'task:D',
        },
        {
            id: 'inoreader',
            kind: 'askWeb',
            typeLabel: '要你去网页完成',
            projectCode: '日',
            projectName: '日常',
            taskTitle: '核实来源',
            title: 'Inoreader 要你登录授权',
            why: '订阅源列表读不到，需要你在浏览器里登录一次。',
            actions: [{ label: '去完成', primary: true }, { label: '不授权' }],
            time: '10:41',
            href: 'task:D2',
        },
    ],
    problems: [
        {
            id: 'p-bing',
            side: 'llm',
            projectName: 'web-search-pro 研究',
            taskTitle: '可能性探索',
            handled: '已转为决定',
            what: '调用 Bing 搜索返回 401：没有 key',
            how: 'Kind 处理不了，已转成上面「等你决定」里的一条。',
            time: '13:47',
            href: 'task:C3',
        },
        {
            id: 'p-restart',
            side: 'aios',
            projectName: null,
            taskTitle: null,
            handled: '处理中',
            what: '后台服务 13:20 没有响应，自动重启了一次',
            how: 'Kind 已核对当时在跑的任务，没有丢进度，正在看重启后的状态。',
            time: '13:20',
        },
        {
            id: 'p-crawl',
            side: 'llm',
            projectName: 'web-search-pro 研究',
            taskTitle: '可行性验证',
            handled: '已处理',
            what: '网页抓取超时 2 次',
            how: 'Kind 换了一个来源重试成功。',
            time: '13:06',
        },
        {
            id: 'p-403',
            side: 'llm',
            projectName: '日常',
            taskTitle: '核实来源',
            handled: '已转为决定',
            what: '2 个简报来源打不开（返回 403）',
            how: 'Kind 用备用来源补回 2 条；是否长期更换这两个来源，已转到「等你决定」。',
            time: '09:02',
            href: 'task:D2',
        },
    ],
    roots: [
        {
            id: 'root',
            projectId: 'wsp',
            projectCode: 'W',
            projectName: 'web-search-pro 研究',
            color: 'var(--line-w)',
            title: '它还有没有价值，值不值得继续',
            line: [
                station('A'),
                { ...station('B'), meta: '2 / 2 项', progress: { done: 2, total: 2 } },
                { ...station('C'), meta: '1 / 3 项', progress: { done: 1, total: 3 } },
                station('D'),
            ],
            attention: '可能性探索 › 要不要申请搜索 key：等你决定',
            flag: { text: '等你 · 2', tone: 'wait' },
        },
        {
            id: 'news',
            projectId: 'news',
            projectCode: 'N',
            projectName: 'news-intelligence',
            color: 'var(--line-n)',
            title: '导入仓库并写结构说明',
            line: [station('N1'), station('N2'), station('N3'), { ...station('N4'), meta: '41 分钟前起' }],
            attention: '导入仓库并写结构说明 › 验收结构说明：等你',
            flag: { text: '等你 · 1', tone: 'wait' },
        },
        {
            id: 'daily',
            projectId: 'daily',
            projectCode: '日',
            projectName: '日常',
            color: 'var(--line-d)',
            title: '每日 AI 简报 · 今天',
            subtitle: '今天',
            line: [station('D1'), station('D2'), station('D3'), station('D4')],
            flag: { text: '有问题 · Kind 处理中', tone: 'fail' },
        },
    ],
    usage: {
        wsp: { model: 'DeepSeek V4 Pro', elapsed: '2 小时 12 分', eta: '≈ 25 分钟', tokens: '31.7k', cost: '¥0.42' },
        news: { model: 'MiniMax M3.1 Flash', elapsed: '22 分钟', eta: '≈ 5 分钟', tokens: '19.4k', cost: '¥0.11' },
        daily: { model: 'MiniMax M3.1 Flash', elapsed: '6 分钟', eta: '≈ 0', tokens: '7.4k', cost: '¥0.04' },
    },
    weekDone: { count: 3, names: ['读清实现与契约', '竞争景观', '多引擎实测'] },
};
/** 现在页顶部的统计：件数按数据算，不写死。 */
export function nowCounts(fixture) {
    const running = fixture.roots.length;
    // 问题行都算（「已转为决定」那一行仍留着，标给用户看）；已处理的单独计数。
    const problems = fixture.problems.length;
    const fixed = fixture.problems.filter((problem) => problem.handled === '已处理').length;
    return { decisions: fixture.decisions.length, problems, fixed, running };
}
