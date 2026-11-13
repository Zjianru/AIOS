import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T039.1/T051.2：任务对话页头的状态与断线自愈。
 * - 页头词表和 Kind 卡同一个 taskStatusOf：写任务 ready=待你核、failed=失败、blocked=卡住了；
 *   接受后=已接受；没有 report 的排队任务=等待中（没有 Attempt 也能读）。
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

test('写任务汇报后页头写「待你核」（和 Kind 卡同一词表），输入框是发送', async ({ page, request }) => {
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
    await expect(state).toHaveText('待你核');
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

/** T051：等任务交报告（task 库的 reportedAt 事实）。 */
async function waitForReport(
  request: import('@playwright/test').APIRequestContext,
  fixture: KernelFixture,
  taskId: string,
): Promise<void> {
  await waitFor('任务交报告', async () => {
    const read = await apiPost(request, fixture, { method: 'task-read', taskId });
    if (!read.ok()) return null;
    const body = (await read.json()) as {
      task: { reportedAt?: number | null; running: boolean };
    };
    return body.task.reportedAt != null && !body.task.running ? true : null;
  });
}

test('任务对话页头：报告 failed → 「失败」（和 Kind 卡同一个词表）', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ taskResult: 'failed' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('帮我查一下新闻归档里的判重规则');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdsOf(fixture, 1);
    await waitForReport(request, fixture, taskId);

    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const state = page.locator('[data-pane="task-conversation"] [data-kind="task-state"]');
    await expect(state).toHaveText('失败', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('任务对话页头：报告 blocked(agent) → 「卡住了」', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ taskResult: 'blocked' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('帮我查一下新闻归档里的判重规则');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdsOf(fixture, 1);
    await waitForReport(request, fixture, taskId);

    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const state = page.locator('[data-pane="task-conversation"] [data-kind="task-state"]');
    await expect(state).toHaveText('卡住了', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('全局暂停时派的写任务：卡和任务对话都「等待中」，开闸后变「待你核」', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    const paused = await apiPost(request, fixture, { method: 'settings-update', paused: true });
    expect(paused.ok()).toBe(true);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('把 hello.txt 的判重改成按 url');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskId] = await taskIdsOf(fixture, 1);

    // Kind 卡：任务还没 Attempt，从 task-read 的事实得到「等待中」。
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card.locator('[data-kind="task-state"]')).toHaveText('等待中', {
      timeout: 30_000,
    });

    // 任务对话：没有事件也要能打开，页头「等待中」，不报错。
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const pane = page.locator('[data-pane="task-conversation"]');
    await expect(pane.locator('[data-kind="task-state"]')).toHaveText('等待中', {
      timeout: 30_000,
    });
    await expect(pane.locator('[data-kind="composer-input"]')).toBeVisible();

    // 开闸：任务开跑并交 ready；写任务当前版本 → 待你核。
    const resumed = await apiPost(request, fixture, { method: 'settings-update', paused: false });
    expect(resumed.ok()).toBe(true);
    await page.goto(urlOf(fixture));
    const finalCard = page.locator('[data-kind="task-result"]').last();
    await expect(finalCard.locator('[data-kind="task-state"]')).toHaveText('待你核', {
      timeout: 60_000,
    });
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(
      page.locator('[data-pane="task-conversation"] [data-kind="task-state"]'),
    ).toHaveText('待你核', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
