import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T098.6：工作页照原型（D124 / reply n 第 2 节）。
 * 标题旁项目筛选「全部 ▾」；右栏「等你」的接受按钮不竖排；底部一句说明；卡片摘要不露 Markdown。
 */

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('工作页：项目筛选下拉、接受按钮横排、底部说明', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work');
    await expect(page.locator('[data-scene-page="work"]')).toBeVisible({ timeout: 30_000 });

    // 项目筛选：默认「全部」的下拉；打开有全部/个人（fixture 没有项目）。
    const filter = page.locator('[data-kind="work-project-filter-select"]');
    await expect(filter).toContainText('全部');
    await filter.click();
    const menu = page.locator('[role="menuitemradio"]');
    await expect(menu.filter({ hasText: '全部' })).toHaveCount(1);
    await expect(menu.filter({ hasText: '个人' })).toHaveCount(1);
    await menu.filter({ hasText: '个人' }).click();
    await expect(page).toHaveURL(/#work\?p=personal/);

    // 右栏「等你」的接受按钮是横排（宽度大于高度），不是被挤成竖排。
    const accept = page.locator('[data-action="wait-accept"]').first();
    await expect(accept).toBeVisible({ timeout: 30_000 });
    await expect(accept).toContainText('接受');
    const box = await accept.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(box!.height);

    // 底部说明在。
    await expect(page.locator('[data-kind="work-board-footer"]')).toContainText('和「现在」的「等你决定」是同一份');
    // 卡片摘要不露 Markdown 原文。
    const text = await page.locator('[data-pane="waiting"]').innerText();
    expect(text).not.toContain('**');
  } finally {
    await fixture.close();
  }
});
