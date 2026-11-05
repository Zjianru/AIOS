import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/// T007.13：假模型让两道 question/asked 分批到达（两条事件间隔 400ms），
/// 页面不白屏、两题都显示，并且能整批作答（T007.11 DoD5 发现的回归）。
test('问题卡分批到达：不白屏、两题都显示', async ({ page }) => {
  const fixture = await startKernelFixture({ askTurn: true, askTurnPaceMs: 400 });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill('问我两个问题');
    await page.getByRole('button', { name: '发送' }).click();

    // 第一题先到；此时草稿只有第一题，第二题还没到。
    await expect(page.getByText('先选一个？', { exact: false })).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-pane="conversation"]')).toBeVisible();

    // 第二题分批到达后，两题都在，页面没有白屏。
    await expect(page.getByText('再自己写一句？', { exact: false })).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="question"]')).toBeVisible();
    await expect(page.getByText('先选一个？', { exact: false })).toHaveCount(1);

    // 整批作答仍然可用。
    await page.getByRole('radio', { name: /Yes/ }).click();
    await page.getByPlaceholder('写下你的回答').fill('分批也不丢');
    await page.getByRole('button', { name: '提交回答' }).click();
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});
