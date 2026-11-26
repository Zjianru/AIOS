import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T098.2：Kind 右栏两块同时在（D124 / reply s 第 2 节）。
 * 聚焦三组（等你 / 出了问题 / 在跑）+「Kind 现在带着」（色条真值、块名照原型、无估算写「还没有分块估算」）
 * + 输入框里的「现在」位置标签。
 */
test('T098.2 Kind 右栏：聚焦三组 + Kind 现在带着 + 页面标签「现在」', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const rail = page.locator('[data-pane="rail"]');
    await expect(rail).toBeVisible();
    await expect(page.locator('[data-pane="conversation"]')).toBeVisible();

    // 上：聚焦三组
    await expect(rail.getByText('聚焦', { exact: true })).toBeVisible();
    for (const group of ['等你', '出了问题', '在跑']) {
      await expect(rail.locator('[data-focus-group="' + group + '"]')).toBeVisible();
    }
    // 空库：三组各自的空文案（首屏没任务时也照实写）。
    await expect(rail.locator('[data-focus-group="等你"]')).toContainText('没有要你的事。');
    await expect(rail.locator('[data-focus-group="出了问题"]')).toContainText('没有出问题。');
    await expect(rail.locator('[data-focus-group="在跑"]')).toContainText('没有在跑。');

    // 下：Kind 现在带着，块名照原型，8 块都写「还没有分块估算」
    await expect(rail.getByText('Kind 现在带着', { exact: true })).toBeVisible();
    const blocks = rail.locator('button[data-context-block]');
    await expect(blocks).toHaveCount(8);
    await expect(rail.getByText('还没有分块估算')).toHaveCount(8);
    await blocks.first().click();
    await expect(rail.getByText('内核还没有给出这一块的大小和条目', { exact: false })).toBeVisible();

    // 输入框里的「现在」位置标签（可去掉）
    const tag = page.locator('[data-kind="composer-page-tag"]');
    await expect(tag).toContainText('现在');
    await tag.getByRole('button', { name: '去掉页面标签' }).click();
    await expect(page.locator('[data-kind="composer-page-tag"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
