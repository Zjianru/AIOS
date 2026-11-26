import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T098.4：记忆页照原型（D124）。
 * 固定头（标题/搜索/五个分节/回 Kind 对话）；条目左右两栏（范围 + 你定的 / Kind 记的）；
 * 做梦面板（开关/间隔可改/马上整理）；隔离区；页脚。
 */

test('记忆页：固定头五节、条目两栏、做梦可改、隔离区、页脚', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#memory');
    const pageRoot = page.locator('[data-kind="memory-page"]');
    await expect(pageRoot).toBeVisible({ timeout: 30_000 });

    // 顶上固定头：标题、搜索、五个分节、回 Kind 对话。
    const header = page.locator('[data-kind="memory-header"]');
    await expect(header.getByText('记忆', { exact: true })).toBeVisible();
    await expect(header.locator('[data-kind="memory-search-input"]')).toBeVisible();
    const tabs = header.locator('[data-memory-tab]');
    await expect(tabs).toHaveCount(5);
    for (const label of ['关于你', '记忆条目', 'Kind 的日记', '做梦', '隔离区']) {
      await expect(header.locator('[data-memory-tab]').filter({ hasText: label })).toHaveCount(1);
    }
    await expect(header.locator('[data-kind="memory-back-kind"]')).toBeVisible();

    // 五个分节都在。
    for (const id of ['about', 'entries', 'diary', 'dream', 'quarantine']) {
      await expect(page.locator('[data-memory-section="' + id + '"]')).toBeVisible();
    }

    // 条目两栏：左范围（个人/项目/角色），右 你定的 / Kind 记的（可错，虚线框）。
    const scopes = page.locator('[data-kind="memory-scopes"]');
    await expect(scopes).toBeVisible();
    await expect(scopes.getByText('个人', { exact: true })).toBeVisible();
    await expect(scopes.getByText('项目', { exact: true })).toBeVisible();
    await expect(scopes.getByText('角色', { exact: true })).toBeVisible();
    await expect(page.locator('[data-kind="memory-group"][data-group="binding"]')).toContainText('你定的');
    await expect(page.locator('[data-kind="memory-group"][data-group="note"]')).toContainText('Kind 记的（可错）');

    // 做梦：开关、间隔可改、马上整理一次；上次/下次。
    const dream = page.locator('[data-kind="memory-dream"]');
    await expect(dream.locator('[data-kind="dream-enabled"]')).toBeVisible();
    await expect(dream.locator('[data-kind="dream-interval"]')).toBeVisible();
    await expect(dream.locator('[data-action="dream-interval-save"]')).toBeVisible();
    await expect(dream.getByText('上次成功', { exact: false })).toBeVisible();

    // 隔离区空态 + 页脚。
    await expect(page.locator('[data-kind="memory-quarantine"]')).toContainText('空。');
    await expect(page.locator('[data-kind="memory-footer"]')).toContainText('记忆都存在这台机器上');

    // 回 Kind 对话。
    await header.locator('[data-kind="memory-back-kind"]').click();
    await expect(page).toHaveURL(/#kind/);
  } finally {
    await fixture.close();
  }
});
