import { test, expect } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T084.4：用量等待的卡/任务页同源文案。
 * 用户路径：派任务 → 长 429 进等待 → 卡和页头同一句 → 刷新仍在 → 到点新 Attempt 行消失。
 */

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';
const CARD_TEXT = '查一下新闻归档里重复链接';

const urlOf = (fixture: KernelFixture) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

function cardOf(page: import('@playwright/test').Page) {
  return page.locator('[data-kind="task-result"]').filter({ hasText: CARD_TEXT }).first();
}

function lineOf(locator: import('@playwright/test').Locator) {
  return locator.locator('[data-kind="task-usage-wait"]');
}

test('长 429：卡和任务页同一句等待行，刷新仍在，新 Attempt 行消失', async ({ page }) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ taskResult: 'usageWait' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const card = cardOf(page);
    await expect(card).toHaveCount(1, { timeout: 30_000 });
    await expect(lineOf(card)).toContainText('用量到上限', { timeout: 30_000 });
    await expect(lineOf(card)).toContainText('自动接着做');
    await expect(card.locator('[data-kind="task-state"]')).toContainText('等待中');

    // 打开任务对话：页头同一句，同一个格式。
    await card.getByRole('button', { name: '打开任务对话' }).click();
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-usage-wait"]')).toContainText('用量到上限', {
      timeout: 30_000,
    });
    const pageLine = await pane.locator('[data-kind="task-usage-wait"]').innerText();
    expect(pageLine).toContain('自动接着做');
    await expect(pane.locator('[data-kind="task-state"]')).toContainText('等待中');

    // 刷新任务页仍在。
    await page.reload();
    await expect(pane.locator('[data-kind="task-usage-wait"]')).toBeVisible({ timeout: 30_000 });
    await expect(pane.locator('[data-kind="task-usage-wait"]')).toHaveText(pageLine);

    // 回 Kind 刷新，卡上仍在，且和页头同一句。
    await pane.getByRole('button', { name: /回 Kind 对话/ }).click();
    await expect(page).toHaveURL(/#kind/);
    await page.reload();
    await expect(lineOf(cardOf(page))).toBeVisible({ timeout: 30_000 });
    expect(await lineOf(cardOf(page)).innerText()).toBe(pageLine);

    // 等 retry-after 到点：新 Attempt 一开，等待词和行都消失。
    await expect(lineOf(cardOf(page))).toHaveCount(0, { timeout: 100_000 });
    await expect(cardOf(page).locator('[data-kind="task-state"]')).not.toContainText('等待中');
  } finally {
    await fixture.close();
  }
});

test('普通 waiting 不出现用量等待行', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ taskResult: 'waiting' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const card = cardOf(page);
    await expect(card).toHaveCount(1, { timeout: 30_000 });
    await expect(card.locator('[data-kind="task-state"]')).toContainText(/等待中|进行中|等报告/);
    await expect(lineOf(card)).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
