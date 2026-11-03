import { test, expect } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;
test.beforeAll(async () => {
  fixture = await startKernelFixture();
});
test.afterAll(async () => {
  await fixture?.close();
});

test('空对话：有说明，不是一片空白', async ({ page }) => {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  const empty = page.locator('[data-kind="empty"]');
  await expect(empty).toBeVisible({ timeout: 20_000 });
  await expect(empty.getByText('这里是你和 Kind 的对话')).toBeVisible();
  await expect(empty.getByText('这条对话一直留着', { exact: false })).toBeVisible();
});
