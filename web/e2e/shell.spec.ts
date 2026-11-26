import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T098.1：全局侧栏照原型（D124）。
 * 顺序：现在 / Kind / 工作 → 项目组 → 记忆 / 监控；底部：外观在上、设置在下。
 * 搜索 ⌘K 不做（reply s）；团队/能力/资料 未接内核不显示（D46）。
 */
test('T098.1 侧栏：主项顺序、项目组位置、底部顺序、记忆书签、无搜索/团队/能力/资料', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const content = page.locator('[data-kind="sidebar-content"]');
    await expect(content).toBeVisible();

    // 主项顺序（跨组，按 DOM 顺序）：现在 → Kind → 工作 → 记忆 → 监控。
    const items = content.locator('button[data-nav]');
    await expect(items).toHaveCount(5);
    expect(await items.evaluateAll((els) => els.map((el) => el.getAttribute('data-nav')))).toEqual([
      'now',
      'kind',
      'work',
      'memory',
      'monitor',
    ]);

    // 项目组落在「工作」之后、「记忆」之前。
    const workBox = await content.locator('[data-nav="work"]').boundingBox();
    const projectBox = await content.getByText('项目', { exact: true }).boundingBox();
    const memoryBox = await content.locator('[data-nav="memory"]').boundingBox();
    expect(workBox).not.toBeNull();
    expect(projectBox).not.toBeNull();
    expect(memoryBox).not.toBeNull();
    expect(projectBox!.y).toBeGreaterThan(workBox!.y);
    expect(projectBox!.y).toBeLessThan(memoryBox!.y);
    // 项目组自带新建入口（+）。
    await expect(content.locator('[data-kind="new-project"]')).toBeVisible();

    // 记忆图标是书签（lucide Bookmark）。
    await expect(content.locator('[data-nav="memory"] svg.lucide-bookmark')).toHaveCount(1);

    // 底部：外观在上、设置在下。
    const footer = page.locator('[data-kind="sidebar-footer"]');
    const footerButtons = footer.locator('button');
    await expect(footerButtons).toHaveCount(2);
    await expect(footerButtons.nth(0)).toContainText('外观');
    await expect(footerButtons.nth(1)).toContainText('设置');

    // 搜索 ⌘K 不做。
    await expect(page.getByText('搜索', { exact: true })).toHaveCount(0);
    await expect(page.getByText('⌘K', { exact: true })).toHaveCount(0);

    // 团队/能力/资料不显示（D46）。
    for (const label of ['团队', '能力', '资料']) {
      await expect(content.getByRole('button', { name: label, exact: true })).toHaveCount(0);
    }
  } finally {
    await fixture.close();
  }
});
