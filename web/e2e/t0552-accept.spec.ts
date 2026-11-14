import { test, expect } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { apiPost, startKernelFixture } from './service-fixture';

/** T055.2：组接受（WIP）。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

const column = (page: import('@playwright/test').Page, key: string) =>
  page.locator('[data-kind="work-board"] section[data-column="' + key + '"]');


function reportCount(fixture: { dataDir: string }, taskId: string): number {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task').all() as { value: string }[];
    const row = rows.find((candidate) => (JSON.parse(candidate.value) as { id: string }).id === taskId);
    return row ? ((JSON.parse(row.value) as { reports?: unknown[] }).reports?.length ?? 0) : 0;
  } finally {
    db.close();
  }
}

function readyTaskCount(fixture: { dataDir: string }): number {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task').all() as { value: string }[];
    return rows.filter(
      (row) => ((JSON.parse(row.value) as { reports?: unknown[] }).reports?.length ?? 0) > 0,
    ).length;
  } finally {
    db.close();
  }
}

async function readyCount(fixture: { baseUrl: string; token: string }): Promise<number> {
  const response = await fetch(fixture.baseUrl + '/api', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ method: 'work' }),
  });
  const body = (await response.json()) as { tasks: { stage: string }[] };
  return body.tasks.filter((task) => task.stage === 'closed').length;
}

test('组接受：勾两份一起接受，回执写在等你里，完成列出现两张', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t055: 'board' });
  try {
    await page.goto(urlOf(fixture));
    const seats = await apiPost(request, fixture, { method: 'settings-update', seats: 4 });
    expect(seats.ok()).toBeTruthy();
    await sendKind(page, '派四个调查任务');
    await expect.poll(() => readyTaskCount(fixture), { timeout: 60_000 }).toBe(4);

    await page.goto(urlOf(fixture, '#work'));
    const waiting = page.locator('[data-pane="waiting"]');
    const items = waiting.locator('[data-kind="wait-item"][data-wait-kind="accept"]');
    await expect(items).toHaveCount(4, { timeout: 30_000 });
    const first = await items.nth(0).locator('[data-kind="wait-title"]').innerText();
    const second = await items.nth(1).locator('[data-kind="wait-title"]').innerText();
    await items.nth(0).locator('[data-kind="wait-pick"]').check();
    await items.nth(1).locator('[data-kind="wait-pick"]').check();
    const acceptSelected = waiting.locator('[data-action="wait-accept-selected"]');
    await expect(acceptSelected).toContainText('接受选中的 2 项');
    await acceptSelected.click();

    const receipt = waiting.locator('[data-kind="accept-receipt"]');
    await expect(receipt).toContainText('接受了 2 项，已关闭', { timeout: 30_000 });
    await expect(receipt.locator('[data-kind="accept-receipt-refused"]')).toHaveCount(0);
    await expect(column(page, 'done')).toContainText(first, { timeout: 10_000 });
    await expect(column(page, 'done')).toContainText(second, { timeout: 10_000 });
    await expect(items).toHaveCount(2, { timeout: 10_000 });
  } finally {
    await fixture.close();
  }
});

test('组接受：勾了以后一项又变了，只记另一项并逐项回原因', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t055: 'board' });
  try {
    await page.goto(urlOf(fixture));
    const seats = await apiPost(request, fixture, { method: 'settings-update', seats: 4 });
    expect(seats.ok()).toBeTruthy();
    await sendKind(page, '派四个调查任务');
    await expect.poll(() => readyTaskCount(fixture), { timeout: 60_000 }).toBe(4);

    await page.goto(urlOf(fixture, '#work'));
    const waiting = page.locator('[data-pane="waiting"]');
    const items = waiting.locator('[data-kind="wait-item"][data-wait-kind="accept"]');
    await expect(items).toHaveCount(4, { timeout: 30_000 });
    const changedTitle = await items.nth(0).locator('[data-kind="wait-title"]').innerText();
    const changedTaskId = await items.nth(0).getAttribute('data-task-id');
    await items.nth(0).locator('[data-kind="wait-pick"]').check();
    await items.nth(1).locator('[data-kind="wait-pick"]').check();
    expect(changedTaskId).toBeTruthy();

    // 在勾选之后让这项再开一次 Attempt（任务对话说一句会开新 Attempt）：
    // 勾时那份报告已经不是最新，接受时单独回原因。
    const continued = await apiPost(request, fixture, {
      method: 'task-say',
      taskId: changedTaskId,
      text: '再核对一遍',
    });
    expect(continued.ok()).toBeTruthy();
    await expect.poll(() => reportCount(fixture, changedTaskId ?? ''), { timeout: 60_000 }).toBe(2);

    const acceptSelected = waiting.locator('[data-action="wait-accept-selected"]');
    await expect(acceptSelected).toContainText('接受选中的 2 项');
    await acceptSelected.click();
    const receipt = waiting.locator('[data-kind="accept-receipt"]');
    await expect(receipt).toContainText('接受了 1 项，已关闭', { timeout: 30_000 });
    await expect(receipt).toContainText('没记 1 项', { timeout: 10_000 });
    await expect(receipt.locator('[data-kind="accept-receipt-refused"]')).toContainText(
      changedTitle,
    );
    await expect(receipt.locator('[data-kind="accept-receipt-refused"]')).toContainText(
      '已经变了',
    );
  } finally {
    await fixture.close();
  }
});
