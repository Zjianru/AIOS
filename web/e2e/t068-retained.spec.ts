import { execFileSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

/** T068.4：报告后保留后台命令：Kind 卡按报告状态、任务页停止按钮、按了进程组没了。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

function taskIdByTitle(fixture: { dataDir: string }, title: string): string {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task').all() as { value: string }[];
    for (const row of rows) {
      const task = JSON.parse(row.value) as { id?: string; title?: string };
      if (task.title === title) return task.id!;
    }
    throw new Error('找不到任务：' + title);
  } finally {
    db.close();
  }
}

function pgidAlive(pid: number): boolean {
  try {
    // kill -0 -pid：进程组在则以 0 退出，不在则抛错。
    execFileSync('kill', ['-0', String(-pid)], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

test('T068 报告后后台还在跑：Kind 卡是报告状态，任务页停止按钮杀掉进程组', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'retained' });
  try {
    // 打开写任务的 bash 开关，再派任务。
    const enabled = await apiPost(request, fixture, { method: 'settings-update', bash: true });
    expect(enabled.ok()).toBeTruthy();
    await page.goto(urlOf(fixture));
    await sendKind(page, '用 bash 起一个后台服务，再报回');
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 60_000 });
    await expect(card).not.toContainText('已派出');
    await expect(card).toContainText('待你核');

    const pidFile = path.join(fixture.dataDir, 'retained-pid');
    await expect.poll(async () => (await import('node:fs')).existsSync(pidFile), { timeout: 30_000 }).toBe(true);
    const pid = Number((await import('node:fs')).readFileSync(pidFile, 'utf8').trim());
    expect(pgidAlive(pid)).toBe(true);

    const taskId = taskIdByTitle(fixture, '用 bash 看一下 hello.txt 再改判重');
    let taskReads = 0;
    page.on('request', (request) => {
      if (request.url().endsWith('/api') && (request.postData() ?? '').includes('"method":"task-read"')) {
        taskReads += 1;
      }
    });
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const stop = page.locator('[data-kind="task-stop-turn"]');
    await expect(stop).toBeVisible({ timeout: 30_000 });
    // T068.4：报告后不再按 600/1600/3200 固定补拉；页面静下来后 3.5 秒不发请求。
    await page.waitForTimeout(1_500);
    const baseline = taskReads;
    await page.waitForTimeout(3_500);
    expect(taskReads).toBe(baseline);
    await expect(stop).toHaveAttribute('aria-label', '停下后台命令');
    const beforeStop = taskReads;
    await stop.click();
    await expect.poll(() => pgidAlive(pid), { timeout: 5_000 }).toBe(false);
    // task-changed 触发一次重读，不是 10 秒兜底/固定补拉。
    await expect.poll(() => taskReads, { timeout: 5_000 }).toBeGreaterThan(beforeStop);
    const afterEvent = taskReads;
    // 兜底窗口走完（10 秒）不应再因定时器多发请求。
    await page.waitForTimeout(10_500);
    expect(taskReads).toBe(afterEvent);
    const read = await apiPost(request, fixture, { method: 'task-read', taskId });
    expect(read.ok()).toBeTruthy();
    const body = (await read.json()) as { task: { backgroundRunning: boolean; running: boolean } };
    expect(body.task.backgroundRunning).toBe(false);
    expect(body.task.running).toBe(false);
    await expect(stop).toHaveCount(0, { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
