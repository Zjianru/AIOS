import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('失败块：凭证原因给「去设置」', async ({ page }) => {
  const fixture = await startKernelFixture({ failure: 'credential' });
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="failure"]')).toBeVisible({ timeout: 30_000 });
    await page.getByRole('button', { name: /去设置/ }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});
