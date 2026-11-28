import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:look';

test('T103.3 外观：主题三选一/跟随系统/减少动效持久化', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-settings="appearance"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

    await page.locator('[data-appearance-theme="light"]').click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await page.reload();
    await expect(page.locator('[data-appearance-theme="light"]')).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

    await page.emulateMedia({ colorScheme: 'light' });
    await page.locator('[data-appearance-theme="system"]').click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await page.emulateMedia({ colorScheme: 'dark' });
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

    await page.locator('[data-appearance-motion]').selectOption('on');
    await expect(page.locator('html')).toHaveAttribute('data-reduced-motion', 'true');
    await page.reload();
    await expect(page.locator('[data-appearance-motion]')).toHaveValue('on');
  } finally {
    await fixture.close();
  }
});
