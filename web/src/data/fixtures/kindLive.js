/**
 * Kind 对话接内核之后要出现的各种状态（T007、T008 的参照）。
 *
 * - 一轮按「步骤」演：工具一条条跑、模型重试、模型提问（等你答）、流式回答、失败。
 * - 提问的形状照内核的 ask_user：问题有 id、可选的短标题、问题本身、可选的选项（label + 一句说明）、
 *   可选的多选；回答是 { id, selected[], custom? }，整批一次交回。不答也是一种回答（模型收到「用户没答」）。
 * - 这些都是假的；真的事件由内核的服务推过来。
 */
const NL = String.fromCharCode(10);
/** 平常发一条消息：读两样东西，再回答。 */
export function defaultScript(user) {
    return {
        user,
        steps: [
            { kind: 'tool', command: 'read README.md', ms: 700, output: '42 行' },
            { kind: 'tool', command: 'bash ls -la /tmp/aios-t006-dod/work', ms: 1200, output: '7 项' },
            {
                kind: 'say',
                ms: 3200,
                text: [
                    '看过了。`work` 里一共 7 项：',
                    '',
                    '- 3 个 Markdown 笔记，最近一次改在昨天；',
                    '- 2 个 CSV，是同一份数据的两个版本；',
                    '- `scripts/` 和 `tmp/` 两个目录。',
                    '',
                    '两个 CSV 只差最后 12 行，要不要我把差异列出来？',
                ].join(NL),
            },
        ],
    };
}
export const DEMO_SCRIPTS = {
    question: {
        user: '帮我把 work 目录整理一下',
        steps: [
            { kind: 'tool', command: 'bash ls -la /tmp/aios-t006-dod/work', ms: 900, output: '7 项' },
            {
                kind: 'ask',
                questions: [
                    {
                        id: 'scope',
                        header: '范围',
                        question: '整理到什么程度？',
                        options: [
                            { label: '只列出来，不动文件（推荐）', description: '先看清楚再决定，什么都不会被移动' },
                            { label: '按类型移进子目录', description: '笔记、数据、脚本各一个目录' },
                        ],
                    },
                    {
                        id: 'name',
                        header: '命名',
                        question: '旧版本的 CSV 放到哪个目录？',
                    },
                ],
                answered: '好，按你说的来：先只列出来，旧版本的 CSV 记在你给的目录名下，等你确认再动。' + NL + NL + '| 类型 | 文件 |' + NL + '|---|---|' + NL + '| 笔记 | 3 个 .md |' + NL + '| 数据 | 2 个 .csv |' + NL + '| 其他 | scripts/、tmp/ |',
                dismissed: '你没回答，我就先不动任何文件，只把现在的样子列出来：' + NL + NL + '- 笔记 3 个、数据 2 个、目录 2 个。' + NL + NL + '想整理的时候再告诉我怎么分。',
            },
        ],
    },
    retry: {
        user: '两个 CSV 差在哪？',
        steps: [
            { kind: 'tool', command: 'bash diff data-v1.csv data-v2.csv', ms: 800, output: '12 行不同' },
            { kind: 'retry', attempt: 1, reason: 'DeepSeek 返回 503（服务繁忙）', ms: 2000 },
            { kind: 'retry', attempt: 2, reason: 'DeepSeek 返回 503（服务繁忙）', ms: 2400 },
            {
                kind: 'say',
                ms: 2200,
                text: '差在最后 12 行：`data-v2.csv` 多了 10 月 29 日到 31 日的记录，其余完全一样。',
            },
        ],
    },
    failCredential: {
        user: '继续',
        steps: [{ kind: 'fail', cause: 'credential', reason: 'DeepSeek 拒绝了这把密钥（401）' }],
    },
    failKeyring: {
        user: '继续',
        steps: [
            {
                kind: 'fail',
                cause: 'keyring',
                reason: '读不到 Keyring 里 DeepSeek 的密钥：密钥文件 ~/.config/aios/keyring.key 不在原处',
            },
        ],
    },
    failModel: {
        user: '把三个笔记合成一篇',
        steps: [
            { kind: 'tool', command: 'read notes/a.md', ms: 600, output: '88 行' },
            { kind: 'retry', attempt: 1, reason: 'DeepSeek 返回 503（服务繁忙）', ms: 1500 },
            { kind: 'retry', attempt: 2, reason: '连接超时', ms: 1500 },
            { kind: 'retry', attempt: 3, reason: 'DeepSeek 返回 503（服务繁忙）', ms: 1500 },
            { kind: 'fail', cause: 'model', reason: 'DeepSeek 连续 4 次没有回应（最后一次：503 服务繁忙）' },
        ],
    },
};
/** 压缩之后对话里那条分隔线展开看到的摘要（假的）。 */
export const COMPACTION_SUMMARY = [
    '## 目标',
    '整理 /tmp/aios-t006-dod/work，先只列出、不动文件。',
    '',
    '## 已经知道的',
    '- 目录里 3 个笔记、2 个 CSV（只差最后 12 行）、scripts/ 和 tmp/。',
    '- 用户希望旧版本的 CSV 放进 archive-2026。',
    '',
    '## 还没做的',
    '- 等用户确认后再移动文件。',
].join(NL);
