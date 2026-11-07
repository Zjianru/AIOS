import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// 设置页「获取可用模型」显示目录窗口；把目录里的新模型加进来时，
/// 四类价格（输入/输出/缓存读/缓存写）跟着列表走，不丢。
test('设置页获取模型：目录窗口和四类价格都在', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    // 目录窗口来自 available-models（camelCase）；deepseek-flash 是 1048576（页面写成 M）。
    await expect(discover.locator('label', { hasText: 'deepseek-flash' })).toContainText('1.048576M');
    // 把目录里多出来的模型勾上：它带着窗口和四类价格进模型列表。
    await discover
      .locator('label', { hasText: 'deepseek-v4-pro' })
      .locator('input[type="checkbox"]')
      .check();
    await editor.getByRole('button', { name: '更新列表' }).click();
    await expect(discover).toHaveCount(0);
    const prices = editor.locator('[data-settings="model-price"]');
    await expect(prices).toHaveCount(2);
    await expect(prices.nth(1)).toContainText('in 0.66');
    await expect(prices.nth(1)).toContainText('out 1.98');
    await expect(prices.nth(1)).toContainText('r 0.022');
    await expect(editor.locator('input[value="1.048576M"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});
