import { test, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

function taskPane(page: Page) {
  return page.locator('[data-pane="task-conversation"]');
}

async function send(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 根会话日志里的 task_accept 回执带任务 id。 */
async function taskIdOf(fixture: KernelFixture): Promise<string> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const root = JSON.parse(
        await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
      ) as { sessionId: string };
      const text = await readFile(
        path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
        'utf8',
      );
      const match = /已派出任务 ([0-9a-f]+)。/.exec(text);
      if (match) return match[1];
    } catch {
      // 会话文件还没写出来；继续等。
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('根会话日志里等不到 task_accept 的任务 id');
}

/** 服务先开出任务会话，页面才打得开。 */
async function waitUntilTaskReadable(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
  taskId: string,
) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const response = await apiPost(request, fixture, { method: 'task-read', taskId });
    if (response.ok()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('TaskRead 一直打不开：' + taskId);
}

async function openTaskChat(page: Page, fixture: KernelFixture, taskId: string) {
  await page.goto(fixture.baseUrl + '/#taskchat:' + taskId);
  const pane = taskPane(page);
  await expect(pane).toBeVisible({ timeout: 20_000 });
  await expect(pane.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
    timeout: 20_000,
  });
  return pane;
}

async function openTaskMenu(page: Page, pane: ReturnType<typeof taskPane>) {
  await pane.locator('[data-kind="task-menu"]').click();
  return page.locator('[role="menu"]');
}

test('在跑任务立即停：页头已停止，Kind 卡已停止，过程还在；停后菜单灰', async ({
  page,
  request,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const taskId = await taskIdOf(fixture);
    await waitUntilTaskReadable(request, fixture, taskId);
    const pane = await openTaskChat(page, fixture, taskId);

    // 等它真的跑起来：页头显示执行中，菜单两项都能点。
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 20_000,
    });
    const menu = await openTaskMenu(page, pane);
    await expect(menu.getByRole('menuitem', { name: /立即停/ })).not.toHaveAttribute(
      'data-disabled',
      '',
    );
    await expect(menu.getByRole('menuitem', { name: /收尾停/ })).not.toHaveAttribute(
      'data-disabled',
      '',
    );
    await expect(menu.getByText('马上掐断，这次 Attempt 记成已停止')).toBeVisible();
    await expect(menu.getByText('做完手上这一步、交一份报告就结束')).toBeVisible();
    await menu.getByRole('menuitem', { name: /立即停/ }).click();

    // 停止落库后被页面读到：页头固定「已停止」。
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('已停止', {
      timeout: 20_000,
    });
    // 停后菜单灰：两项都点不了。
    const stoppedMenu = await openTaskMenu(page, pane);
    await expect(stoppedMenu.getByRole('menuitem', { name: /立即停/ })).toHaveAttribute(
      'data-disabled',
      '',
    );
    await expect(stoppedMenu.getByRole('menuitem', { name: /收尾停/ })).toHaveAttribute(
      'data-disabled',
      '',
    );
    await page.keyboard.press('Escape');

    // 过程还在：被停下的这一轮仍画在任务对话里。
    await expect(pane.locator('[data-kind="process"]').first()).toBeVisible({
      timeout: 20_000,
    });

    // Kind 对话里出现固定「已停止」的结果卡。
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card).toHaveAttribute('data-state', 'stopped');
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('已停止');
    await expect(card.locator('[data-kind="task-title"]')).toContainText('新闻归档');

    // 刷新后页头和卡片都还在（事实来自任务库/会话日志，不是页面内存）。
    await page.goto(fixture.baseUrl + '/#taskchat:' + taskId);
    await expect(taskPane(page).locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    await page.reload();
    const again = taskPane(page);
    await expect(again.locator('[data-kind="task-state"]')).toHaveText('已停止', {
      timeout: 20_000,
    });
    await expect(again.locator('[data-kind="process"]').first()).toBeVisible();
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('没在跑的任务：任务菜单两项都灰', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'ready' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await card.getByRole('button', { name: /打开任务对话/ }).click();
    const pane = taskPane(page);
    await expect(pane.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    const menu = await openTaskMenu(page, pane);
    await expect(menu.getByRole('menuitem', { name: /立即停/ })).toHaveAttribute(
      'data-disabled',
      '',
    );
    await expect(menu.getByRole('menuitem', { name: /收尾停/ })).toHaveAttribute(
      'data-disabled',
      '',
    );
  } finally {
    await fixture.close();
  }
});

test('收尾停：点完提示「收尾停已插进去」', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const taskId = await taskIdOf(fixture);
    await waitUntilTaskReadable(request, fixture, taskId);
    const pane = await openTaskChat(page, fixture, taskId);
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('执行中', {
      timeout: 20_000,
    });
    const menu = await openTaskMenu(page, pane);
    await menu.getByRole('menuitem', { name: /收尾停/ }).click();
    await expect(page.getByText('收尾停已插进去', { exact: true })).toBeVisible({
      timeout: 20_000,
    });
    // 收尾停后还能改立即停：菜单仍在。
    await expect(pane.locator('[data-kind="task-menu"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('设置 › 执行：席位 1–16 保存并刷新，0/17 拒存；暂停刷新还在', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:exec');
    const section = page.locator('[data-kind="exec-settings"]');
    await expect(section).toBeVisible({ timeout: 20_000 });
    const seats = section.locator('[data-kind="seats"]');
    await expect(seats).toHaveValue('3');

    // 0 / 17 不保存：提示后回到已保存的值。
    await seats.fill('0');
    await seats.press('Enter');
    await expect(page.getByText('席位要在 1–16 之间', { exact: false })).toBeVisible({
      timeout: 10_000,
    });
    await expect(seats).toHaveValue('3');
    await seats.fill('17');
    await seats.press('Enter');
    await expect(seats).toHaveValue('3');

    // 1 保存后刷新还在。
    await seats.fill('1');
    await seats.press('Enter');
    await expect(seats).toHaveValue('1');
    await page.reload();
    await expect(page.locator('[data-kind="exec-settings"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="seats"]')).toHaveValue('1');

    // 暂停开关保存后刷新还在。
    const pause = page.locator('[data-kind="global-pause"]');
    await expect(pause).toHaveAttribute('aria-checked', 'false');
    await pause.click();
    await expect(pause).toHaveAttribute('aria-checked', 'true');
    await page.reload();
    await expect(page.locator('[data-kind="exec-settings"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="global-pause"]')).toHaveAttribute(
      'aria-checked',
      'true',
    );
  } finally {
    await fixture.close();
  }
});