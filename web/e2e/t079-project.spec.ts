import { test, expect, type Page } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * 项目第一版：侧栏新建/项目页逐项改/工作页 ?p= 筛选/监控 ?p= 与「个人」。
 * 任务先用 Kind 派一张，再用真协议 task_set_project 归到项目；页面只读服务端事实。
 */

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

async function createProject(page: Page, name: string, goal: string, dir: string) {
  await page.locator('[data-kind="new-project"]').click();
  await expect(page.locator('[data-kind="new-project-dialog"]')).toBeVisible();
  await page.locator('[data-kind="new-project-name"]').fill(name);
  await page.locator('[data-kind="new-project-goal"]').fill(goal);
  await page.locator('[data-kind="new-project-dir"]').fill(dir);
  await page.locator('[data-kind="new-project-submit"]').click();
  await expect(page.locator('[data-page="project"]')).toBeVisible({ timeout: 20_000 });
  return new URL(page.url()).hash.slice('#project:'.length);
}

test('项目：侧栏新建、项目页逐项改、工作页 ?p=、监控 ?p= 与个人', async ({ page, request }) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    // 先派一张任务，任务本身是个人；后面用真协议改归属。
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });

    await page.goto(urlOf(fixture, '#work'));
    const projectId = await createProject(
      page,
      '清河县农技站',
      '把农技站的事管起来',
      '~/projects/qinghe',
    );
    expect(projectId).toBeTruthy();
    await expect(page.locator('[data-kind="project-name"]')).toContainText('清河县农技站');
    await expect(page.locator('[data-kind="project-goal"]')).toContainText('把农技站的事管起来');
    await expect(page.locator('[data-kind="sidebar-project"]')).toContainText('清河县农技站');
    await expect(page.locator('[data-kind="project-empty"]')).toBeVisible();

    // 新建对话框重名拒绝（不建第二个同名项目）。
    await page.locator('[data-kind="new-project"]').click();
    await page.locator('[data-kind="new-project-name"]').fill('清河县农技站');
    await page.locator('[data-kind="new-project-submit"]').click();
    await expect(page.locator('[data-kind="new-project-error"]')).toContainText('已经存在', {
      timeout: 20_000,
    });
    await page.getByRole('button', { name: '取消', exact: true }).click();

    // 真任务归到项目；项目页出现任务（当时在等你接受）。
    const workResponse = await apiPost(request, fixture, { method: 'work' });
    const work = (await workResponse.json()) as {
      tasks: { id: string; title: string }[];
    };
    const task = work.tasks.find((item) => item.title.includes('判重改成按'));
    expect(task, 'Kind 派出的任务应在工作快照里').toBeTruthy();
    const assigned = await apiPost(request, fixture, {
      method: 'task_set_project',
      taskId: task!.id,
      projectId,
    });
    expect(assigned.ok()).toBeTruthy();
    await page.goto(urlOf(fixture, '#project:' + projectId));
    await expect(page.locator('[data-page="project"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="project-task"]').filter({ hasText: '判重改成按' })).toBeVisible({
      timeout: 20_000,
    });

    // 名字/目标/目录逐项改：先改目标。
    await page.locator('button[data-kind="project-edit-goal"]').click();
    const goalInput = page.locator('input[data-kind="project-edit-goal"]');
    await goalInput.fill('把农技站的事一件件收掉');
    await goalInput.press('Enter');
    await expect(page.locator('[data-kind="project-goal"]')).toContainText('一件件收掉', { timeout: 20_000 });

    // 默认目录也能改；存的是绝对路径。
    await page.locator('button[data-kind="project-edit-dir"]').click();
    const dirInput = page.locator('input[data-kind="project-edit-dir"]');
    await dirInput.fill('/tmp/qinghe');
    await dirInput.press('Enter');
    await expect(page.locator('[data-kind="project-dir"]')).toContainText('/tmp/qinghe', {
      timeout: 20_000,
    });

    // 重名拒绝：另建一个项目，再把它改成第一个项目已用的名字。
    const second = await apiPost(request, fixture, {
      method: 'project_create',
      name: '别的项目',
      goal: '',
    });
    expect(second.ok()).toBeTruthy();
    await page.goto(urlOf(fixture, '#project:' + projectId));
    await page.locator('button[data-kind="project-edit-name"]').click();
    const nameInput = page.locator('input[data-kind="project-edit-name"]');
    await nameInput.fill('别的项目');
    await nameInput.press('Enter');
    await expect(page.locator('[data-kind="project-save-error"]')).toContainText('已经存在', {
      timeout: 20_000,
    });
    // 刷新后名字没被改坏。
    await page.reload();
    await expect(page.locator('[data-kind="project-name"]')).toContainText('清河县农技站');

    // 工作页按项目筛；个人筛看不到它。
    await page.goto(urlOf(fixture, '#work?p=' + encodeURIComponent(projectId)));
    await expect(page.locator('[data-kind="work-project-filter"]')).toContainText('清河县农技站');
    const visibleTask = page
      .locator('[data-kind="work-card"], [data-kind="board-wait-item"]')
      .filter({ hasText: '判重改成按' });
    await expect(visibleTask).toHaveCount(1, { timeout: 20_000 });
    await page.goto(urlOf(fixture, '#work?p=personal'));
    await expect(page.locator('[data-kind="work-project-filter"]')).toContainText('个人');
    await expect(
      page.locator('[data-kind="work-card"], [data-kind="board-wait-item"]').filter({ hasText: '判重改成按' }),
    ).toHaveCount(0);

    // 监控按项目筛；没归属的任务显示「个人」。
    await page.goto(urlOf(fixture, '#monitor?p=' + encodeURIComponent(projectId)));
    await expect(page.locator('[data-kind="monitor-project-trigger"]')).toContainText('清河县农技站');
    const taskCard = page.locator('[data-kind="task-card"]').filter({ hasText: '判重改成按' }).first();
    await expect(taskCard).toBeVisible({ timeout: 30_000 });
    await expect(taskCard.locator('[data-kind="task-project"]')).toContainText('清河县农技站');
    // 诊断页也带项目名（monitor_facts 的 task info project_id）。
    await taskCard.click();
    await expect(page.locator('[data-kind="diagnose-project"]')).toContainText('清河县农技站', {
      timeout: 20_000,
    });
    await page.goto(urlOf(fixture, '#monitor?p=' + encodeURIComponent(projectId)));
    await page.locator('[data-kind="monitor-project-trigger"]').click();
    await page.getByRole('menuitemradio', { name: '个人' }).click();
    await expect(page.locator('[data-kind="monitor-project-trigger"]')).toContainText('个人');
    await expect(page.locator('[data-kind="task-card"]').filter({ hasText: '判重改成按' })).toHaveCount(0);
    await expect(page.locator('[data-kind="project-empty"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});
