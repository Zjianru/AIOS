/**
 * Kind 右栏「Kind 现在带着」的样例（experience-spec-v1.md §6.2）。
 *
 * 真实实现里：各块大小按字数估算（每 4 个字约 1 token），合计是服务商报的实际数；
 * 所以这里也把「合计」和「各块估算」分开：reportedTotal = 服务商报的 46.2k，
 * 各块 tokens 是估算、加起来必须等于 reportedTotal。改了任何一块以后，合计先显示成估算。
 *
 * 角色「让他离开」、资料「拿掉」是 §6.2 允许的就地调整；记忆「更正」要跳记忆页（第 3 期），
 * 这里先只读，写进第 2 期取舍。
 */
function block(key, name, unit, items, extra) {
    const tokens = items.reduce((sum, item) => sum + item.tokens, 0);
    return { key, name, unit, items, tokens, ...extra };
}
function same(label, tokens, action) {
    return { id: label, label, tokens, action };
}
export const kindContext = {
    reportedTotal: 46200,
    blocks: [
        block('base', '基本设定', null, [same('AIOS 里的话，说人话；不确定就说不确定', 800)]),
        block('abilities', '能力说明', '项', [
            same('网页搜索', 340),
            same('读取网页', 340),
            same('写作文档（Word）', 340),
            same('每日简报', 340),
            same('日历唤醒', 340),
            same('命令执行', 340),
            same('代码审查', 340),
            same('GitHub 连接', 340),
            same('Bing 搜索', 340),
            same('数据去重脚本', 340),
        ]),
        block('roles', '角色', '位', [
            same('研究员', 1050, 'leave'),
            same('评测员', 1050, 'leave'),
            same('审查员', 1050, 'leave'),
            same('源码解读者', 1050, 'leave'),
        ]),
        block('memories', '记忆', '条', [
            same('简报每条附原文链接', 180),
            same('先找一手来源', 180),
            same('结论要带证据', 180),
            same('不写套话', 180),
            same('不确定就说不确定', 180),
        ]),
        block('materials', '资料', '份', [
            same('我的简介.md', 1260, 'remove'),
            same('写作规范.md', 1260, 'remove'),
            same('web-search-pro README', 1260, 'remove'),
            same('Brave Search API 定价', 1260, 'remove'),
            same('搜索引擎评测方法.pdf', 1260, 'remove'),
            same('简报来源清单.xlsx', 1260, 'remove'),
            same('news-intelligence 仓库', 1260, 'remove'),
            same('外包协议草案.docx', 1260, 'remove'),
            same('Semafor 订阅页', 1260, 'remove'),
            same('竞品首页截图.png', 1260, 'remove'),
        ]),
        block('annotations', '你发的批注', '处', []),
        block('history', '对话历史', '轮', [same('最近 5 轮（完整）', 3000), same('更早 1995 轮', 15200)], { count: 2000, note: '其中 3 天前的已压缩成摘要' }),
        block('tools', '工具结果', null, [
            same('网页搜索 12 次', 2400),
            same('读取网页 8 次', 1800),
            same('命令执行 3 次', 1200),
            same('写文件 2 次', 700),
        ]),
    ],
};
