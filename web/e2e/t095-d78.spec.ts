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

async function answerQuestions(page: import('@playwright/test').Page) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const asking = page.getByText('想先问你一件事');
    if ((await asking.count()) === 0) return;
    const radios = page.getByRole('radio');
    if ((await radios.count()) > 0) {
      try {
        await radios.first().click({ timeout: 3_000 });
      } catch {
        // 卡在重渲染：下一轮再试。
      }
    } else {
      const custom = page.locator('[placeholder="自己写"]');
      if ((await custom.count()) > 0) {
        try {
          await custom.fill('只是记一下', { timeout: 3_000 });
        } catch {
          // 下一轮再试。
        }
      }
    }
    const submit = page.locator('[data-action="question-answer"]');
    if ((await submit.count()) > 0) {
      try {
        await submit.click({ timeout: 5_000, force: true });
      } catch {
        await page.waitForTimeout(300);
      }
    }
    await page.waitForTimeout(400);
  }
}

/** 真 key 的 Kind 可能先出提问卡：答题后再等这一轮收尾。 */
async function waitAnswer(page: import('@playwright/test').Page) {
  for (let attempt = 0; attempt < 8; attempt += 1) {
    await answerQuestions(page);
    const done = page.getByRole('button', { name: /已完成|已派出|停下了|没送到|失败了/ }).last();
    try {
      await expect(done).toBeVisible({ timeout: attempt === 0 ? 180_000 : 60_000 });
      break;
    } catch (error) {
      if (attempt === 7) throw error;
    }
  }
  // 真 key 这一步还没完全收尾时按 Esc 会先当成「停止」；等停止按钮消失再操作菜单。
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, {
    timeout: 120_000,
  });
}

async function openRewindMenu(page: import('@playwright/test').Page) {
  const menu = page.locator('[data-kind="rewind-menu"]');
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await input(page).click();
    await page.keyboard.press('Escape');
    await page.waitForTimeout(120);
    await page.keyboard.press('Escape');
    try {
      await expect(menu).toBeVisible({ timeout: 5_000 });
      return menu;
    } catch {
      await page.waitForTimeout(300);
    }
  }
  await expect(menu).toBeVisible({ timeout: 20_000 });
  return menu;
}

test('T095 D78：恢复对话后新问题不提被收起的暗号', async ({ page }) => {
  const fixture = await startD78KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '蓝鲸七号');
    await waitAnswer(page);
    await send(page, '再记一句：今天先到这里。');
    await waitAnswer(page);

    // 双击 Esc 开菜单，选带暗号的那句恢复。
    const menu = await openRewindMenu(page);
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
    await send(
      page,
      '只凭我们现在的对话回答：你还记得我最早发过的那句话吗？没印象就说「没有」。不要用任何工具、记忆或日志去找。',
    );
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