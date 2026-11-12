import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T039.1：任务对话页头的状态与断线自愈。
 * - 汇报后页头必须写「已汇报 · 待你核」，输入框是发送（纸飞机）。
 * - 任务在跑时断网 → 期间任务被停掉 → 恢复网络：重连后页头刷新为真实状态、按钮变回发送。
 */

const TITLE = '把 hello.txt 的判重改成按 url';

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function rootLogText(fixture: KernelFixture): Promise<string> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  return readFile(path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'), 'utf8');
}

/** 根会话日志里所有派出的任务 id（按派出顺序）。 */
async function taskIdsOf(fixture: KernelFixture, count: number): Promise<string[]> {
  for (let attempt = 0; attempt < 600; attempt += 1) {
    const ids = [...(await rootLogText(fixture)).matchAll(/已派出任务 ([0-9a-f]+)。/g)].map(
      (match) => match[1],
    );
    if (ids.length >= count) return ids.slice(0, count);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('等不到 ' + count + ' 个任务 id');
}

async function waitFor<T>(label: string, check: () => Promise<T | null | false>): Promise<T> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < 900; attempt += 1) {
    try {
      const value = await check();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('等不到：' + label + (lastError ? '（最后错误：' + String(lastError) + '）' : ''));
}

test('任务汇报后页头写「已汇报 · 待你核」，输入框是发送', async ({ page, request }) => {
  test.setTimeout(150_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdsOf(fixture, 1);
    // 等任务真的交报告（task 库的 reportedAt）。
    await waitFor('任务交报告', async () => {
      const read = await apiPost(request, fixture, { method: 'task-read', taskId });
      if (!read.ok()) return null;
      const body = (await read.json()) as { task: { reportedAt?: number | null; running: boolean } };
      return body.task.reportedAt != null && !body.task.running ? true : null;
    });

    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 30_000 });
    const state = page.locator('[data-pane="task-conversation"] [data-kind="task-state"]');
    await expect(state).toHaveCount(1, { timeout: 30_000 });
    await expect(state).toContainText('已汇报 · 待你核');
    // 不在跑：按钮是发送（纸飞机），不是「停下这一轮」。
    await expect(page.locator('[data-kind="task-send"]')).toBeVisible();
    await expect(page.locator('[data-kind="task-stop-turn"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('任务在跑时断网、期间任务被停掉，恢复网络后页头自愈为「已停止」且按钮变回发送', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t029Restart: 'main' });
  try {
    await apiPost(request, fixture, { method: 'settings-update', seats: 8 });
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('帮我派四个任务');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskA] = await taskIdsOf(fixture, 4);

    await page.goto(urlOf(fixture, '#taskchat:' + taskA));
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 30_000 });
    const state = page.locator('[data-pane="task-conversation"] [data-kind="task-state"]');
    await expect(state).toContainText('执行中', { timeout: 30_000 });
    await expect(page.locator('[data-kind="task-stop-turn"]')).toBeVisible();

    // 断网（页面 WS 断开），期间任务被停掉。
    await page.context().setOffline(true);
    const stopped = await apiPost(request, fixture, {
      method: 'task-stop',
      taskId: taskA,
      mode: 'now',
    });
    expect(stopped.ok()).toBeTruthy();
    // 恢复网络：不刷新页面，等页面自己重连并把页头事实拉回来。
    await page.context().setOffline(false);
    await expect(state).toContainText('已停止', { timeout: 60_000 });
    await expect(page.locator('[data-kind="task-send"]')).toBeVisible();
    await expect(page.locator('[data-kind="task-stop-turn"]')).toHaveCount(0);
  } finally {
    await page.context().setOffline(false);
    await fixture.close();
  }
});
