import { test, expect, type Page } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

/**
 * T098.6：工作页照原型（D124 / reply n 第 2 节）。
 * 标题旁项目筛选「全部 ▾」；右栏「等你」的接受按钮不竖排；底部一句说明；卡片摘要不露 Markdown。
 */

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('工作页：项目筛选下拉、接受按钮横排、底部说明', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: 'T098 筛选项目',
      goal: '验证工作页按项目过滤',
    });
    expect(created.ok()).toBeTruthy();
    const projectBody = (await created.json()) as { project: { id: string } };
    const projectId = projectBody.project.id;
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

    // 右栏「等你」的接受按钮是横排（宽度大于高度），「看交付记录」也在同一行、不被挤下去。
    const accept = page.locator('[data-action="wait-accept"]').first();
    await expect(accept).toBeVisible({ timeout: 30_000 });
    await expect(accept).toContainText('接受');
    const box = await accept.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(box!.height);
    const see = page.locator('[data-action="wait-open"]').first();
    await expect(see).toContainText('看交付记录');
    const seeBox = await see.boundingBox();
    expect(seeBox).not.toBeNull();
    expect(Math.abs(seeBox!.y - box!.y)).toBeLessThan(8);
    expect(seeBox!.x).toBeGreaterThan(box!.x);

    // 各项目档：选项出现、选它后地址变、看板和右栏真的过滤（个人任务和它的接受项都不见了）。
    await page.locator('[data-kind="work-project-filter-select"]').click();
    await page.locator('[role="menuitemradio"]').filter({ hasText: 'T098 筛选项目' }).click();
    await expect(page).toHaveURL(new RegExp('#work\\?p=' + projectId));
    await expect(
      page.locator('[data-column="doing"]').getByText('把 hello.txt 的判重改成按 url'),
    ).toHaveCount(0);
    await expect(page.locator('[data-action="wait-accept"]')).toHaveCount(0);
    await expect(page.locator('[data-pane="waiting"]')).toContainText('没有要你的事');

    // 回到个人：个人任务和接受项都回来（右栏与看板联动）。
    await expect(page.locator('[role="menuitemradio"]')).toHaveCount(0);
    await page.locator('[data-kind="work-project-filter-select"]').click();
    const personalItem = page.locator('[role="menuitemradio"]').filter({ hasText: '个人' });
    await expect(personalItem).toBeVisible();
    await personalItem.click();
    await expect(page).toHaveURL(/#work\?p=personal/);
    await expect(page.locator('[data-action="wait-accept"]')).toHaveCount(1);

    // 底部说明在「等你」栏底部（不是看板底部），文案照原型。
    await expect(page.locator('[data-kind="waiting-footer"]')).toContainText('和「现在」的「等你决定」是同一份');
    await expect(page.locator('[data-kind="waiting-footer"]')).toContainText('判定、确认、接续在 Kind 对话的卡上做');
    await expect(page.locator('[data-kind="work-board-footer"]')).toHaveCount(0);
    // 卡片摘要不露 Markdown 原文。
    const text = await page.locator('[data-pane="waiting"]').innerText();
    expect(text).not.toContain('**');
  } finally {
    await fixture.close();
  }
});
