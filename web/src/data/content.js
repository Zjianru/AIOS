export const NL = String.fromCharCode(10);
export const md = (lines) => lines.join(NL);
/**
 * 生成 2000 轮 Kind 对话用的内容池。
 * 内容混着来（需求 §二）：短答、长 markdown、表格、代码块、工具调用、
 * v10-tables §2.2 的各类卡片、思考摘要、引用片。
 */
export const USER_SAY = [
    '帮我看一下 web-search-pro 的进展。',
    '这件事先停一下，我明天再决定。',
    '把多引擎实测的结论写成一段，我发给同事看。',
    'news-intelligence 那个仓库读完了吗？',
    '今天简报里为什么少了三条？',
    '给我一份同类方案的对比，只要结论和依据。',
    '这个站点的预估时间准吗？',
    '把「给出判断」这件事的进度列一下。',
    '研究报告里那两个来源打不开，换个方式再试。',
    '我下午三点半回来，到点提醒我看对比矩阵。',
    '这套流程能不能固定下来，以后都按这个走？',
    '不要每次都问我，这种只读的东西直接做就行。',
    '把刚才那段整理成表格。',
    '算了，先按原来的安排继续。',
    '这件事交给研究员，评测员盯着验收。',
    '我想看看这条地铁线现在走到哪一站了。',
    '帮我记一下：以后简报不要超过 12 条。',
    '上次那个去重脚本还在吗？',
    '把结构说明的第三部分再展开一点。',
    '为什么这一步会打回？',
    '能不能分两条线并行做？',
    '把失败的原因写清楚，不要只写结论。',
    '这个里程碑的标准我觉得还差一条。',
    '帮我对照一下这两份资料的差异。',
    '这周一共花了多少？',
    '我改了顺序，新的排在前面。',
    '这个能力现在能直接用吗？',
    '把这个结果收进资料，别的项目也能用。',
    '今天的活儿先做到这儿。',
    '这条线先挂起，等我有空再说。',
    '把这件事的账算一下。',
    '再跑一次，我要看两次的差别。',
    '我刚把文件拖进来了，你读一下。',
    '别用 MiniMax 了，这段换 DeepSeek。',
    '晚上八点提醒我复看。',
    '这一段看不懂，换成大白话。',
    '把每一步谁做、交出什么写出来。',
    '这个结论有依据吗？是哪几段？',
    '三个引擎的结果为什么不一样？',
    '帮我把这一批批注发出去。',
    '这个站为什么一直不动？',
];
export const SHORT_ANSWER = [
    '好，接着原来的安排往下走。这一步不用你管，做完我在「现在」里给你看结果。',
    '看过了，进度没变：多引擎实测还在等 Bing 的 key，其余的都在动。',
    '这件事我按只读处理，不再打断你。真需要你确认的时候再叫你。',
    '可以。我把它记成一条规矩，以后这类情况都这么办。',
    '现在的判断还没到能下的时候，证据只有 2 个引擎的实测。',
    '那一站已经完成了，结论写在「评测记录.md」里，v2 是复核之后的那一版。',
    '我先停在这里。等你回来，从这条分隔线往下看就是新的。',
    '换成了备用来源，简报还是缺 1 条，我在「出了问题」里记了一笔。',
    '不用重跑。上次的结果我留着，两次的差别我列出在下面。',
    '这属于改东西的动作，我需要你点一下才动。',
];
export const THINKING_SUMMARY = [
    '先确认用户指的是哪一件事，再去读它的最新状态，只汇报变化。',
    '用户要的是判断，不是报告。判断需要三类证据：同类方案、实测、还能做什么。',
    '这一条属于只读操作，按用户以前的说法可以直接做，不打断他。',
    '如果他指的就是刚才那张卡，直接改那张卡，不再发第二张。',
    '先把范围收窄到这一站，别把整条线的历史都翻出来。',
    '需要估算的地方标 ≈，并且把怎么估的说清楚。',
];
export const THINKING_FULL = [
    '第 1 步：把用户这句话里的指代落到具体对象上。第 2 步：读对象的当前状态，只取和上一条消息之后的变化。第 3 步：判断有没有需要用户决定的地方；有的话给选项和推荐，没有就直接往下做。第 4 步：答复里先给结论。',
    '用户问的是「进展」，不是「重做」。所以只读、只汇报。要避免把整条时间线倒一遍，那样他看不过来。挑三件：正在推进的、打完的、等他的。',
    '这里有一个坑：用户说「不要每次都问我」，但这次的动作是写文件。按规矩，写文件要问。用户的话覆盖不了这一类，先在答复里说明原因，再请他点一下。',
    '数字要分开画。估算的数前面加 ≈ 并用虚线样式，实际数正常写。合计是实际的、各块是估算的，这两种不能混成一个数。',
];
export const LONG_MD = [
    [
        '## 结论',
        '',
        '这一类比今天仍然有价值，但价值不在「多一个搜索入口」，在**作为别的能力的兜底**。三条依据：',
        '',
        '1. **同类方案已经很密。** 三家托管搜索 API 的免费额度合计覆盖了大部分日常查询，单独做一个搜索入口没有位置。',
        '2. **实测里能跑通的部分不差。** 三个引擎跑同一组 20 个查询，命中率与耗时见下表。',
        '3. **真正的缺口在离线与私有环境。** 这一点托管 API 补不上。',
        '',
        '## 建议',
        '',
        '保留一个窄用途：当别的 skill 需要「没有 key 也能搜一下」时兜底。不做通用入口。',
    ],
    [
        '## 这一步做了什么',
        '',
        '- 把 4 个引擎的查询集统一成同一组 20 个问题；',
        '- 每个引擎跑满 20 个，记录命中与耗时；',
        '- 对同一 URL 做规范化去重，再统计命中率；',
        '- 把「缺 Bing」这一项单独标出来，不混进总计。',
        '',
        '## 卡在哪',
        '',
        'Bing 需要 key。没有 key 的情况下我不编造第 4 个引擎的数字，结论里会注明缺这一项。',
    ],
    [
        '## 为什么这条线慢',
        '',
        '不是模型慢，是三个来源里有两个返回 403。重试两次仍然失败，我换成备用来源补回了两条，还剩一条没有替代。',
        '',
        '按规矩，这一类「要不要长期换来源」属于要你决定的事，我把它放到「等你决定」，不自己定。',
    ],
];
export const MARKDOWN_TABLE = [
    [
        '| 引擎 | 命中 | 平均耗时 | 备注 |',
        '|---|---|---|---|',
        '| Google | 18 / 20 | 0.9s | 免费额度够用 |',
        '| DuckDuckGo | 16 / 20 | 1.4s | 2 次超时后重试成功 |',
        '| Brave | 17 / 20 | 0.7s | 需要 key |',
        '| Bing | — | — | 缺 key，未测 |',
    ],
    [
        '| 方案 | 维护成本 | 离线可用 | 结论 |',
        '|---|---|---|---|',
        '| 现有 skill | 低 | 否 | 保留 |',
        '| 托管搜索 API | 低 | 否 | 补位 |',
        '| 自建抓取 | 高 | 是 | 只在兜底场景用 |',
    ],
    [
        '| 项 | 值 | 来源 |',
        '|---|---|---|',
        '| 免费额度 | 2,000 次 / 月 | 订阅页 |',
        '| 单价 | ≈ ¥0.003 / 次 | 估算 |',
        '| 本月用量 | 1,284 次 | 实际 |',
    ],
];
export const CODE_BLOCK = [
    [
        '去重脚本只做了三件事：规范化 URL、丢掉跟踪参数、按首路径去重。',
        '',
        '```ts',
        'export function dedupe(urls: string[]): string[] {',
        '  const seen = new Set<string>();',
        '  const out: string[] = [];',
        '  for (const raw of urls) {',
        '    const u = new URL(raw);',
        '    u.hash = "";',
        '    for (const k of [...u.searchParams.keys()]) {',
        '      if (k.startsWith("utm_")) u.searchParams.delete(k);',
        '    }',
        '    const key = u.host + u.pathname;',
        '    if (seen.has(key)) continue;',
        '    seen.add(key);',
        '    out.push(u.toString());',
        '  }',
        '  return out;',
        '}',
        '```',
        '',
        '跑完从 61 条降到 48 条，重复的 13 条全部是同一篇文章的不同入口。',
    ],
    [
        '结构说明里「怎么运行」那一段我实际跑过：',
        '',
        '```bash',
        'npm ci',
        'node scripts/doctor.mjs          # 检查依赖与配置',
        'node scripts/bootstrap.mjs --dry # 不写盘，只打印',
        '```',
        '',
        'doctor 通过；bootstrap --dry 报 2 个可选依赖缺失，不影响主流程。',
    ],
];
export const TOOL_STEPS = [
    { command: 'node scripts/doctor.mjs', cwd: '~/news-intelligence', out: 'ok · 12 checks passed', exit: 0, ms: 1240 },
    { command: 'node scripts/bootstrap.mjs --dry', cwd: '~/news-intelligence', out: '2 optional deps missing: sharp, canvas', exit: 0, ms: 2110 },
    { command: 'git rev-parse HEAD', cwd: '~/news-intelligence', out: '9c1f2aa', exit: 0, ms: 60 },
    { command: 'git ls-files | wc -l', cwd: '~/news-intelligence', out: '1284', exit: 0, ms: 90 },
    { command: 'rg -n "export default" --glob "src/**" | wc -l', cwd: '~/news-intelligence', out: '34', exit: 0, ms: 120 },
    { command: 'node -e "console.log(engines)"', cwd: '~/web-search-pro', out: 'google / duckduckgo / brave / bing (needs key)', exit: 0, ms: 340 },
];
export const TOOL_CALLS = [
    ['read_file', 'news-intelligence/README.md'],
    ['read_file', 'web-search-pro/scripts/doctor.mjs'],
    ['read_dir', 'news-intelligence/src'],
    ['web_fetch', 'brave.com/search/api'],
    ['grep', 'dedupe'],
    ['material_read', '搜索引擎评测方法.pdf · §3'],
];
export const APPROVAL_COMMANDS = [
    'rm -rf ~/web-search-pro/.cache/engines',
    'git -C ~/news-intelligence checkout -b read/structure-notes',
    'python3 scripts/dedupe.py --in results/raw.json --out results/deduped.json',
    'npm i --save-dev sharp canvas',
];
export const MEMORY_NOTES = [
    { note: '简报不超过 12 条，每条一句话 + 链接。', noteKind: '用户声明' },
    { note: '只读类动作不必每次确认，写盘前必须确认。', noteKind: '用户声明' },
    { note: 'web-search-pro 的窄用途是「别的 skill 的兜底搜索」。', noteKind: '判断' },
    { note: '评测前先确认各引擎免费额度，不足时减少复测轮数。', noteKind: '事实' },
    { note: 'The Information 与 Semafor 会返回 403。', noteKind: '事实' },
    { note: '结论里不能出现没实测过的引擎数字。', noteKind: '判断' },
    { note: '批注只留还没发出去的，发出去就在对话里。', noteKind: '用户声明' },
];
export const DECISION_OPTIONS = [
    [
        { label: '申请 Bing 的 key，补测第 4 个引擎', description: '结论更完整，但要等审批，今天出不来。' },
        { label: '先不申请，结论里注明缺这一项', description: '今天能出结论，代价是覆盖面少一块。' },
    ],
    [
        { label: '长期换成备用来源', description: '简报不再缺条，但备用来源时效差一些。' },
        { label: '维持原来的来源，缺的条目照实写', description: '来源不变，简报偶尔少 1–3 条。' },
    ],
    [
        { label: '把这个窄用途定成规矩', description: '以后别的 skill 需要兜底搜索时直接用，不再问你。' },
        { label: '每次都先问', description: '你多一次点击，但每一步都在你眼前。' },
    ],
];
export const ARTIFACT_NAMES = [
    '评测记录.md',
    '结构说明.docx',
    '同类方案对比矩阵.xlsx',
    '契约清单.pdf',
    '基线结果.json',
    '今日简报.md',
    '模块依赖图.png',
];
export const WAKE_TEXTS = [
    { when: '今天 15:30', todo: '看对比矩阵草稿，顺便重试早上失败的两个来源。' },
    { when: '每天 09:00', todo: '生成 AI 新闻简报，12 条以内。' },
    { when: '周日 20:00', todo: '提醒「给出判断」里程碑还有 3 天到期。' },
    { when: '明天 10:00', todo: '把多引擎实测的复核结论复看一遍。' },
];
export const PROBLEMS = [
    { what: 'The Information 与 Semafor 返回 403，重试 2 次仍失败。', handled: '已处理' },
    { what: '后台服务 13:20 没有响应，已自动重启一次。', handled: '已处理' },
    { what: '评测记录第 3 个引擎的结果没有去重，命中率被高估。', handled: '已转为决定' },
    { what: 'Bing 引擎缺少 key，第 4 个引擎无法实测。', handled: '处理中' },
];
