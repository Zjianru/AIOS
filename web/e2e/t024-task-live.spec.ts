import { test, expect, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';
const FINAL = '已经上报。';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

function taskPane(page: Page) {
  return page.locator('[data-pane="task-conversation"]');
}

async function send(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 根会话日志里的 task_accept 回执带任务 id；任务对话要在它出现后立刻打开。 */
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

/** 任务刚开始时会话日志可能还没建好：先用 TaskRead 探到能打开，再开页面。 */
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

function watchTaskReads(page: Page): string[] {
  const reads: string[] = [];
  page.on('response', (response) => {
    if (!response.url().endsWith('/api')) return;
    if (!(response.request().postData() ?? '').includes('"method":"task-read"')) return;
    void response
      .text()
      .then((text) => reads.push(text))
      .catch(() => {});
  });
  return reads;
}

async function openTaskChat(
  page: Page,
  fixture: KernelFixture,
  taskId: string,
  reads: string[],
) {
  await page.goto(fixture.baseUrl + '/#taskchat:' + taskId);
  const pane = taskPane(page);
  await expect(pane).toBeVisible({ timeout: 20_000 });
  await expect(pane.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
    timeout: 20_000,
  });
  await expect
    .poll(() => reads.length, { timeout: 20_000 })
    .toBeGreaterThan(0);
  return pane;
}

test('在跑时打开：新工具调用和回答不用刷新出现，各只一次；离开再进不重复', async ({
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

    const reads = watchTaskReads(page);
    const pane = await openTaskChat(page, fixture, taskId, reads);

    // 打开时这份快照必须早于工具调用和最后回答；否则「后来出现」不能证明是实时帧。
    expect(reads[0]).not.toContain('"tool/call"');
    expect(reads[0]).not.toContain(FINAL);

    // 工具调用：工作过程组出现，点开能看到 read 这一行。
    const process = pane.locator('[data-kind="process"]').first();
    await expect(process).toBeVisible({ timeout: 30_000 });
    await process.locator('[data-kind="process-group"] button').first().click();
    await expect(pane.locator('[data-kind="tool-row"][data-tool="read"]')).toBeVisible({
      timeout: 30_000,
    });

    // 回答：流式文字随后落成日志里的最后一条，刷新之外不用点任何东西。
    await expect(pane.getByText(FINAL, { exact: true })).toHaveCount(1, {
      timeout: 30_000,
    });
    await expect(pane.getByText('工作中')).toHaveCount(0, { timeout: 30_000 });
    await expect(process).toContainText('已完成', { timeout: 20_000 });

    // 一次连接、一次 TaskRead：期间没有重拉快照，也没有「刷新」按钮。
    expect(reads.length, reads.join('\n---\n')).toBe(1);
    await expect(pane.getByRole('button', { name: /刷新/ })).toHaveCount(0);
    await expect(page.locator('[data-kind="composer"]')).toHaveCount(0);

    // 离开再进：任务已经收尾，只补齐快照；内容各只出现一次。
    await pane.getByRole('button', { name: /回 Kind 对话/ }).click();
    await expect(page).toHaveURL(/#kind/);
    await page.goto(fixture.baseUrl + '/#taskchat:' + taskId);
    const again = taskPane(page);
    await expect(again.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    await expect(again.getByText(FINAL, { exact: true })).toHaveCount(1, {
      timeout: 20_000,
    });
    const againProcess = again.locator('[data-kind="process"]').first();
    await expect(againProcess).toContainText('已完成');
    await againProcess.locator('button').first().click();
    await againProcess.locator('[data-kind="process-group"] button').first().click();
    await expect(again.locator('[data-kind="tool-row"][data-tool="task_report"]')).toBeVisible();
    await expect(again.getByText(FINAL, { exact: true })).toHaveCount(1);
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('换任务：旧连接的帧不进新页面', async ({ page, request }) => {
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '帮我查一下新闻归档里的判重规则');
    const taskId = await taskIdOf(fixture);
    await waitUntilTaskReadable(request, fixture, taskId);
    const reads = watchTaskReads(page);
    const pane = await openTaskChat(page, fixture, taskId, reads);
    await expect(pane.locator('[data-kind="process"]').first()).toBeVisible({
      timeout: 30_000,
    });

    // 同一场景换到另一个任务：旧连接必须关掉，旧任务的后续帧不能出现在新页面。
    await page.evaluate(() => {
      window.location.hash = 'taskchat:no-such-task';
    });
    const fallback = page.locator('[data-kind="task-read-fallback"]');
    await expect(fallback).toBeVisible({ timeout: 20_000 });
    // 等旧任务在后台跑完（一帧都不该进来）。
    await page.waitForTimeout(6000);
    await expect(fallback).toBeVisible();
    await expect(page.getByText(FINAL, { exact: true })).toHaveCount(0);

    // 回到原任务：刷新式地重开也能看到完整内容。
    await page.evaluate((id) => {
      window.location.hash = 'taskchat:' + id;
    }, taskId);
    const again = taskPane(page);
    await expect(again.locator('[data-kind="task-full-title"]')).toHaveText(TITLE, {
      timeout: 20_000,
    });
    await expect(again.getByText(FINAL, { exact: true })).toHaveCount(1, {
      timeout: 20_000,
    });
  } finally {
    await fixture.close();
  }
});
