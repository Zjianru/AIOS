import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T041：Kind 任务卡状态与任务对话同源（task-read）。
 * 用户路径：派任务 → 任务对话点「停下这一轮」（只中断当前轮、不发 relay）→ 回 Kind（不刷新）→ 卡状态变「等待中」。
 */

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function cardOf(page: import('@playwright/test').Page) {
  return page.locator('[data-kind="task-result"]').filter({ hasText: TITLE }).first();
}

function taskStateOf(page: import('@playwright/test').Page) {
  return cardOf(page).locator('[data-kind="task-state"]');
}

test('派任务→任务对话点「停下这一轮」→回 Kind：卡状态变等待中（不刷新）', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(cardOf(page)).toHaveCount(1, { timeout: 30_000 });
    await expect(taskStateOf(page)).toContainText('进行中', { timeout: 30_000 });

    // 进任务对话，点「停下这一轮」。
    await cardOf(page).getByRole('button', { name: '打开任务对话' }).click();
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane).toBeVisible({ timeout: 20_000 });
    await expect(pane.locator('[data-kind="task-state"]')).toContainText('执行中', {
      timeout: 30_000,
    });
    await page.keyboard.press('Escape');
    await expect(pane.getByText(/停下了这一轮/)).toBeVisible({ timeout: 20_000 });
    await pane.getByRole('button', { name: /回 Kind 对话/ }).click();
    await expect(page).toHaveURL(/#kind/);

    // 不刷新：Kind 卡状态跟着任务库变「等待中」（红：还停在「进行中」）。
    await expect(taskStateOf(page)).toContainText('等待中', { timeout: 20_000 });
    await expect(taskStateOf(page)).not.toContainText('进行中');
  } finally {
    await fixture.close();
  }
});

test('立即停：Kind 卡状态变已停止', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(cardOf(page)).toHaveCount(1, { timeout: 30_000 });
    await expect(taskStateOf(page)).toContainText('进行中', { timeout: 30_000 });
    // 从任务菜单「立即停」（等价接口）。
    const taskId = await cardOf(page)
      .getByRole('button', { name: '打开任务对话' })
      .getAttribute('data-task-id')
      .catch(() => null);
    void taskId;
    // 卡上没暴露 task id：从根日志里取（测试辅助）。
    const { readFile } = await import('node:fs/promises');
    const path = await import('node:path');
    let id = '';
    const rootFile = path.join(fixture.dataDir, 'kind-root.json');
    for (let i = 0; i < 600 && !id; i += 1) {
      const root = JSON.parse(await readFile(rootFile, 'utf8')) as { sessionId: string };
      const text = await readFile(path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'), 'utf8');
      const match = /已派出任务 ([0-9a-f]+)。/.exec(text);
      if (match) id = match[1];
      else await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const stopped = await apiPost(request, fixture, { method: 'task-stop', taskId: id, mode: 'now' });
    expect(stopped.ok()).toBeTruthy();
    await expect(cardOf(page)).toHaveAttribute('data-state', 'stopped', { timeout: 20_000 });
    await expect(taskStateOf(page)).toContainText('已停止');
  } finally {
    await fixture.close();
  }
});

test('汇报后：Kind 卡状态仍是 relay 的结果状态（有结果/待你核）', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(cardOf(page)).toHaveCount(1, { timeout: 30_000 });
    await expect(cardOf(page)).toHaveAttribute('data-state', /ready|review/, { timeout: 60_000 });
    await expect(taskStateOf(page)).toContainText(/有结果|待你核/);
  } finally {
    await fixture.close();
  }
});
