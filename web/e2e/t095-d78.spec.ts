//! T095.6 D78：真 key（MiniMax）恢复对话后发新问题，回答不提被收起的那段。
//! 无 key 时跳过，由 D78 环境跑（key 从 ~/.zshrc 运行时取，不落文件/命令行/证据）。
import { expect, test } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(300_000);

const input = (page: import('@playwright/test').Page) =>
  page.locator('[data-kind="composer-input"]');
const conversation = (page: import('@playwright/test').Page) =>
  page.locator('[data-pane="conversation"]');

async function send(page: import('@playwright/test').Page, text: string) {
  await input(page).fill(text);
  await page.keyboard.press('Enter');
}

async function waitAnswer(page: import('@playwright/test').Page) {
  await expect(
    page.getByRole('button', { name: /已完成|已派出|停下了|没送到/ }).last(),
  ).toBeVisible({ timeout: 240_000 });
}

test('T095 D78：恢复对话后新问题不提被收起的暗号', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '请记住一个暗号：蓝鲸七号。只回「收到」。');
    await waitAnswer(page);
    await send(page, '再记一句：今天先到这里。');
    await waitAnswer(page);

    // 双击 Esc 开菜单，选带暗号的那句恢复。
    await input(page).click();
    await page.keyboard.press('Escape');
    await page.waitForTimeout(120);
    await page.keyboard.press('Escape');
    const menu = page.locator('[data-kind="rewind-menu"]');
    await expect(menu).toBeVisible({ timeout: 20_000 });
    await menu.locator('[data-kind="rewind-row"]').filter({ hasText: '蓝鲸七号' }).click();
    await expect(menu).toHaveAttribute('data-phase', 'actions');
    await menu.locator('[data-kind="rewind-action"][data-action="restore"]').click();
    await expect(menu).toHaveCount(0);
    // 收起段直接不见；原文回到输入框。
    await expect(
      conversation(page).locator('[data-kind="user-bubble"]').filter({ hasText: '蓝鲸七号' }),
    ).toHaveCount(0);
    await expect(input(page)).toHaveValue(/蓝鲸七号/);

    // 新问题：被收起的暗号不在模型上下文里，回答不能提它。
    await send(page, '我们刚才约好的暗号是什么？不知道就直说不知道。');
    await waitAnswer(page);
    const answerText = await conversation(page).innerText();
    expect(answerText).not.toContain('蓝鲸七号');

    // 日志事实：恢复事件落盘（区间藏掉）。
    const root = JSON.parse(
      await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
    ) as { sessionId: string };
    const log = await readFile(
      path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
      'utf8',
    );
    expect(log).toContain('"type":"conversation/rewound"');
  } finally {
    await fixture.close();
  }
});
