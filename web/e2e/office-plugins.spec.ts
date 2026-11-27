//! 设置 › 插件页（官方 Office 卡、开关当场生效、内容包空态）。

import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('设置 › 插件：Office 卡默认关，打开/关掉当场生效并持久化', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#settings:plugins`);
    const card = page.locator('[data-kind="plugin-card"]');
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toContainText('Office');
    await expect(card).toContainText('官方插件');
    await expect(card.locator('[data-kind="plugin-state"]')).toHaveText('已关闭');
    await expect(card.locator('[data-kind="plugin-effect"]')).toContainText('打开影响');
    await expect(card).toContainText('Skill 摘要');
    await expect(card).toContainText('数据目录');
    await expect(card).toContainText('plugins/office');
    await expect(page.locator('[data-kind="content-packs-empty"]')).toContainText('还没有导入的包');

    await page.locator('[data-kind="plugin-toggle"]').click();
    await expect(card.locator('[data-kind="plugin-state"]')).toHaveText('已开启', { timeout: 20_000 });
    await page.reload();
    const reloaded = page.locator('[data-kind="plugin-card"]');
    await expect(reloaded.locator('[data-kind="plugin-state"]')).toHaveText('已开启', {
      timeout: 30_000,
    });

    await page.locator('[data-kind="plugin-toggle"]').click();
    await expect(page.locator('[data-kind="plugin-state"]')).toHaveText('已关闭', {
      timeout: 20_000,
    });
    await page.reload();
    await expect(page.locator('[data-kind="plugin-state"]')).toHaveText('已关闭', {
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});
