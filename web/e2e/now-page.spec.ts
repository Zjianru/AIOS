import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T098.3：现在页照原型（D124 / reply s 第 2 节）。
 * 顶部通知说明、三组空状态、页尾两句；有任务时按总任务一张卡（站点链/卡在哪句/模型·已用·token·花费，
 * 「做到几成」「还需」空着）；等你决定摘要不露 Markdown。
 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('现在页：顶部通知说明、空状态、页尾两句', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });

    // 顶上：「什么时候会通知你」可展开说明。
    const notify = page.locator('[data-kind="now-notify"]');
    await expect(notify).toBeVisible();
    await notify.click();
    await expect(page.locator('[data-kind="now-notify-body"]')).toContainText('等你决定和出问题会提醒');
    await notify.click();

    // 空库：三组都不出卡，只写都在正常推进；周末完成行和页尾两句在。
    await expect(page.locator('[data-kind="now-empty"]')).toContainText('都在正常推进');
    await expect(page.locator('[data-kind="now-week-done"]')).toBeVisible();
    await expect(page.getByText('AIOS 概念 · 现在', { exact: false })).toBeVisible();
    await expect(page.getByText('「还需」带 ≈ 的是估算', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('现在页：有任务时按总任务一张卡，摘要不露 Markdown', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture, '#kind'));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const result = page.locator('[data-kind="task-result"]').last();
    await expect(result).toBeVisible({ timeout: 60_000 });

    await page.goto(urlOf(fixture, '#now'));
    await expect(page.locator('[data-scene-page="now"]')).toBeVisible({ timeout: 30_000 });
    // 任务落在「在推进」或「等你决定」，两种都算接了现有事实。
    const advancingCard = page.locator('[data-kind="now-advancing-card"]');
    const waitItem = page.locator('[data-kind="wait-item"]');
    await expect
      .poll(async () => (await advancingCard.count()) + (await waitItem.count()), { timeout: 30_000 })
      .toBeGreaterThan(0);
    // 等你决定的摘要不露 Markdown 原文。
    const decisions = page.locator('[data-kind="now-decisions"]');
    if ((await decisions.count()) > 0) {
      const text = await decisions.innerText();
      expect(text).not.toContain('**');
      expect(text).not.toMatch(/^#+ /m);
    }
  } finally {
    await fixture.close();
  }
});
