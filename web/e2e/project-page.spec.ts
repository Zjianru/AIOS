import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

/**
 * T098.5：项目页 + 新建项目对话框照原型（D124 / reply s 第 2 节）。
 * 跟 Kind 说、目标/默认目录说明/建于、项目记忆真数、监控入口、任务区、在工作页看；
 * 新建项目对话框字段/按钮。
 */

test('项目页：跟 Kind 说、目录说明/建于、项目记忆/监控入口、任务区', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({});
  try {
    const created = await apiPost(request, fixture, {
      method: 'project_create',
      name: 'T098 项目页对照',
      goal: '把项目页照原型补齐',
    });
    expect(created.ok()).toBeTruthy();
    const body = (await created.json()) as { project: { id: string } };
    const projectId = body.project.id;

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#project:' + projectId);
    await expect(page.locator('[data-page="project"]')).toBeVisible({ timeout: 30_000 });

    await expect(page.locator('[data-kind="project-kind-say"]')).toContainText('跟 Kind 说');
    // 大标题 + 一句目标 + 入口一行（不是标签表单/三 pill）。
    await expect(page.locator('[data-kind="project-name"]')).toContainText('T098 项目页对照');
    await expect(page.getByText('把项目页照原型补齐', { exact: false })).toBeVisible();
    await expect(page.getByText('派任务时预填成任务的目录', { exact: false })).toBeVisible();
    const today = new Date();
    await expect(page.locator('[data-kind="project-created-at"]')).toHaveText(
      today.getMonth() + 1 + ' 月 ' + today.getDate() + ' 日',
    );
    await expect(page.locator('[data-kind="project-memory-count"]')).toHaveAttribute('data-project-memory-mine', '0');
    await expect(page.locator('[data-kind="project-memory-count"]')).toHaveAttribute('data-project-memory-kinds', '0');
    await expect(page.locator('[data-kind="project-memory-link"]')).toContainText('项目记忆');
    await expect(page.locator('[data-kind="project-monitor-link"]')).toContainText('监控');
    await expect(page.locator('[data-kind="project-work-link"]')).toContainText('在工作页看这个项目');
    await expect(page.locator('[data-kind="project-empty"]')).toContainText('这个项目还没有任务');

    // 「目标」「默认目录」是原型的一行可改字：点笔出输入框，Esc 退出不保存。
    await page.locator('[data-kind="project-edit-goal"]').click();
    const goalInput = page.locator('[data-kind="project-goal"] input');
    await expect(goalInput).toHaveValue('把项目页照原型补齐');
    await goalInput.press('Escape');
    await page.locator('[data-kind="project-edit-dir"]').click();
    const dirInput = page.locator('[data-kind="project-dir"] input');
    await expect(dirInput).toHaveAttribute('placeholder', '不设');
    await dirInput.press('Escape');

    // 在工作页看这个项目：带项目筛选过去。
    await page.locator('[data-kind="project-work-link"]').click();
    await expect(page).toHaveURL(/#work\?p=/);

    // 跟 Kind 说：回 Kind。
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#project:' + projectId);
    await page.locator('[data-kind="project-kind-say"]').click();
    await expect(page).toHaveURL(/#kind/);
  } finally {
    await fixture.close();
  }
});

test('新建项目对话框：名字/一句目标/默认目录、取消/创建', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({});
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work');
    await expect(page.locator('[data-scene-page="work"]')).toBeVisible({ timeout: 30_000 });
    await page.locator('[data-kind="new-project"]').click();
    const dialog = page.locator('[data-kind="new-project-dialog"]');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('新建项目');
    await expect(dialog).toContainText('名字必有；目标和默认目录可以以后改');
    const nameInput = dialog.locator('[data-kind="new-project-name"]');
    const goalInput = dialog.locator('[data-kind="new-project-goal"]');
    const dirInput = dialog.locator('[data-kind="new-project-dir"]');
    await expect(nameInput).toBeVisible();
    await expect(goalInput).toBeVisible();
    await expect(goalInput).toHaveAttribute('placeholder', '这个项目要变成什么');
    await expect(dirInput).toBeVisible();
    await expect(dirInput).toHaveAttribute('placeholder', '~/projects/...');
    await expect(dialog.getByText('取消', { exact: true })).toBeVisible();
    await expect(dialog.getByText('创建', { exact: true })).toBeVisible();

    // 走创建：建完跳项目页，名字/目标都在。
    await nameInput.fill('对话框建的项目');
    await goalInput.fill('建完能跳进去');
    await dialog.locator('[data-kind="new-project-submit"]').click();
    await expect(page).toHaveURL(/#project:/, { timeout: 30_000 });
    await expect(page.locator('[data-page="project"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="project-name"]')).toContainText('对话框建的项目');
    await expect(page.getByText('建完能跳进去', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});
