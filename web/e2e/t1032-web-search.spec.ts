//! T103.2：联网搜索服务商页（20 家、选一家、key 只写不读、地址/检测）。
import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model';

test('T103.2 联网搜索：20 家目录、选 Exa、填 key、改地址、检测', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ fakeExa: true });
  try {
    await page.goto(urlOf(fixture));
    const section = page.locator('[data-settings="web-search"]');
    await expect(section).toBeVisible({ timeout: 30_000 });
    await expect(section.locator('[data-web-provider]')).toHaveCount(20, { timeout: 20_000 });
    await expect(section.locator('[data-settings="web-search-status"]')).toContainText('还没有选服务商');

    await section.locator('[data-web-select]').selectOption('exa');
    await expect(section.locator('[data-web-host]')).toHaveValue('https://api.exa.ai');
    await section.locator('[data-web-host]').fill(fixture.exa!.baseUrl);
    await section.locator('[data-web-host-save]').click();
    await expect(section.locator('[data-web-host]')).toHaveValue(fixture.exa!.baseUrl);

    await section.locator('[data-web-key-input]').fill('exa-secret');
    await section.locator('[data-web-key-save]').click();
    await expect(section.locator('[data-settings="web-search-status"]')).toContainText('能用', { timeout: 20_000 });
    await section.locator('[data-web-check]').click();
    await expect(section.locator('[data-settings="web-search-detail"]')).toContainText('上次检查', { timeout: 20_000 });

    // key 写入后页面不回显；刷新后仍然只显示「key 已存」。
    const input = section.locator('[data-web-key-input]');
    await expect(input).toHaveValue('');
    await page.reload();
    await expect(page.locator('[data-settings="web-search"]')).toContainText('key 已存', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
