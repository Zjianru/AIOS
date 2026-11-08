import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T017.3：工作过程里的搜索卡，以及不认识的工具退回通用输入/输出卡（票面验收二）。
test('工作过程卡片：搜索卡、不认识的工具退回通用卡，不报错', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ toolCards: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('搜一下文件，再调一个没见过的工具');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });

    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    const rows = process.locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText('搜索文件');
    await expect(rows.nth(1)).toContainText('工具调用');

    // 搜索卡：真实 glob 搜到夹具写的两个文件。
    await rows.nth(0).locator('button').first().click();
    await expect(rows.nth(0).getByText('2 个文件')).toBeVisible();
    await expect(rows.nth(0).getByText('a.ts', { exact: true })).toBeVisible();

    // 不认识的工具：通用卡的输入 / 输出，页面不报错。
    await rows.nth(1).locator('button').first().click();
    await expect(rows.nth(1).getByText('输入', { exact: true })).toBeVisible();
    await expect(rows.nth(1).getByText('输出', { exact: true })).toBeVisible();
    await expect(rows.nth(1).getByText('"note": "hi"', { exact: false })).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

/// T017.3：工作过程里提问那一行点开，看到问题和我的回答（故事 7：从 tool/* 信封渲染）。
test('工作过程提问卡：从信封渲染，问题和回答都在', async ({ page }) => {
  const fixture = await startKernelFixture({ askTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('问我两个问题');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await page.getByRole('radio', { name: /Yes/ }).click();
    await page.getByPlaceholder('写下你的回答').fill('我自己写的答');
    await page.getByRole('button', { name: '提交回答' }).click();
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 20_000 });

    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    const row = process.locator('[data-kind="tool-row"]').first();
    await expect(row).toContainText('提问');
    await row.locator('button').first().click();
    // 收成回执：回执一行、问题 → 回答两行，都在这张工作过程卡里。
    await expect(row.getByText('你回答了 Kind 的 2 个问题')).toBeVisible();
    await expect(row.getByText('先选一个？', { exact: false })).toBeVisible();
    await expect(row.getByText('Yes', { exact: true })).toBeVisible();
    await expect(row.getByText('我自己写的答', { exact: true })).toBeVisible();
  } finally {
    await fixture.close();
  }
});
