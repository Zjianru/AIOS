import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

async function send(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/**
 * T098.4：记忆页照原型（D124）。
 * 固定头（标题/搜索/五个分节/回 Kind 对话）；条目左右两栏（范围 + 你定的 / Kind 记的）；
 * 做梦面板（开关/间隔可改/马上整理）；隔离区；页脚。
 */

test('记忆页：固定头五节、条目两栏、做梦可改、隔离区、页脚', async ({ page }) => {
  test.setTimeout(180_000);
  // memoryFlow：真走一遍「记住/记一笔」，让记忆页有可展开的条目（不是伪造响应）。
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').waitFor({ timeout: 30_000 });
    await send(page, '记住：回答先给结论');
    await expect(page.locator('[data-memo-kind="remember"]').first()).toBeVisible({ timeout: 30_000 });
    await send(page, '记一下你看到的编码问题');
    await expect(page.locator('[data-memo-kind="note"]').first()).toBeVisible({ timeout: 30_000 });
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
    await expect(page.locator('[data-kind="memory-about"]')).toContainText('回答先给结论');

    // 条目展开：点开看全文 + 来源/版本（T098.4 F2 补测）。
    const noteGroup = page.locator('[data-kind="memory-group"][data-group="note"]');
    const expand = noteGroup.locator('[data-kind="memory-expand"]').first();
    await expect(expand).toBeVisible({ timeout: 30_000 });
    await expand.click();
    await expect(noteGroup.getByText(/来源 .* · 第 \d+ 版/)).toBeVisible();

    // 做梦：开关、间隔可改、马上整理一次；上次/下次。
    const dream = page.locator('[data-kind="memory-dream"]');
    await expect(dream.locator('[data-kind="dream-enabled"]')).toBeVisible();
    await expect(dream.locator('[data-kind="dream-interval"]')).toBeVisible();
    await expect(dream.locator('[data-action="dream-interval-save"]')).toBeVisible();
    await expect(dream.getByText('上次成功', { exact: false })).toBeVisible();

    // 做梦间隔可改：改 12 小时保存，重载后仍是 12（T098.4 F2 补测）。
    const interval = dream.locator('[data-kind="dream-interval"]');
    const before = await interval.inputValue();
    const next = before === '12' ? '24' : '12';
    await interval.fill(next);
    await dream.locator('[data-action="dream-interval-save"]').click();
    await page.reload();
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
    await expect(
      page.locator('[data-kind="memory-dream"] [data-kind="dream-interval"]'),
    ).toHaveValue(next);

    // 隔离区空态 + 页脚。
    await expect(page.locator('[data-kind="memory-quarantine"]')).toContainText('空。');
    await expect(page.locator('[data-kind="memory-footer"]')).toContainText('记忆都存在这台机器上');

    // 固定头两行（标题/搜索一行，五个分节 + 回 Kind 一行）。
    const titleBox = await header.locator('[data-kind="memory-title"]').boundingBox();
    const searchBox = await header.locator('[data-kind="memory-search-input"]').boundingBox();
    const navBox = await header.locator('[data-memory-tab]').first().boundingBox();
    expect(titleBox).not.toBeNull();
    expect(searchBox).not.toBeNull();
    expect(navBox).not.toBeNull();
    expect(Math.abs(searchBox!.y - titleBox!.y)).toBeLessThan(12);
    expect(navBox!.y).toBeGreaterThan(titleBox!.y + 8);

    // 「加一条」没有页面直写协议：带一句草稿回 Kind 输入框，由对话写记忆。
    await page.locator('[data-kind="memory-add-about"]').click();
    await expect(page).toHaveURL(/#kind/);
    await expect(page.locator('[data-kind="composer-input"]')).toHaveValue('记住：', {
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});
