import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T039.2：Kind 对话里的任务卡任何时刻都有明确状态词，且状态随任务变化。
 * 覆盖：派发→进行中→待你核→接受后「已关闭」；停止→「已停止」；所有卡都有 task-state。
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

/** 所有任务卡都有非空状态词。 */
async function expectEveryCardHasState(page: import('@playwright/test').Page): Promise<void> {
  const cards = page.locator('[data-kind="task-result"]');
  const count = await cards.count();
  expect(count).toBeGreaterThan(0);
  for (let index = 0; index < count; index += 1) {
    const status = cards.nth(index).locator('[data-kind="task-state"]');
    await expect(status).toHaveCount(1);
    expect(((await status.textContent()) ?? '').trim().length).toBeGreaterThan(0);
  }
}

test('任务卡状态：进行中→待你核→已关闭，每张卡都有状态词', async ({ page }) => {
  test.setTimeout(150_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const cards = page.locator('[data-kind="task-result"]');
    // 派发后先有进行中的卡（很快会变待你核，先断言「有卡且状态词非空」和最终的待你核）。
    await expect(cards).toHaveCount(1, { timeout: 30_000 });
    await expect(cards.locator('[data-kind="task-state"]')).toContainText(/进行中|待你核/, {
      timeout: 30_000,
    });
    const review = page.locator('[data-kind="task-result"][data-state="review"]');
    await expect(review).toHaveCount(1, { timeout: 30_000 });
    await expect(review.locator('[data-kind="task-state"]')).toContainText('待你核');
    await expectEveryCardHasState(page);
    // T054：接受以后任务关闭，状态词变成「已关闭」。
    await review.locator('[data-kind="task-accept"]').click();
    await expect(review.locator('[data-kind="task-state"]')).toContainText('已关闭', {
      timeout: 30_000,
    });
    await expectEveryCardHasState(page);
  } finally {
    await fixture.close();
  }
});

test('任务卡状态：停止的任务显示「已停止」，有结果/卡住各有着落', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t029Restart: 'main' });
  try {
    await apiPost(request, fixture, { method: 'settings-update', seats: 8 });
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('帮我派四个任务');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const [taskA] = await taskIdsOf(fixture, 4);
    const stopped = await apiPost(request, fixture, {
      method: 'task-stop',
      taskId: taskA,
      mode: 'now',
    });
    expect(stopped.ok()).toBeTruthy();
    const cards = page.locator('[data-kind="task-result"]');
    await expect(cards).toHaveCount(4, { timeout: 60_000 });
    // 被停掉的那张卡：已停止。
    await expect(
      page.locator('[data-kind="task-result"][data-state="stopped"]').filter({ hasText: '任务甲' }),
    ).toHaveCount(1, { timeout: 60_000 });
    await expectEveryCardHasState(page);
    // 其余三张也都有状态词（等报告/有结果/卡住等）。
    const statuses = await cards.locator('[data-kind="task-state"]').allTextContents();
    for (const status of statuses) expect(status.trim().length).toBeGreaterThan(0);
  } finally {
    await fixture.close();
  }
});
