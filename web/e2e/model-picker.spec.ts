import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('底栏选模型：下一轮请求用新模型', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.locator('[data-kind="model-picker"]').click();
    await page.getByRole('menuitem', { name: /DeepSeek Pro/ }).click();
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect.poll(() => fixture.lastModel(), { timeout: 30_000 }).toBe('deepseek-pro');
  } finally {
    await fixture.close();
  }
});
