import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T042：任务对话里跑的是 ThinAgent，运行中过程行应写「ThinAgent 工作中」；
 * Kind 对话里仍是「Kind 工作中」。只改运行中文案，完成态不动。
 */

const TITLE = '查一下新闻归档里重复链接的判重规则和影响范围';
const CARD_TEXT = '查一下新闻归档里重复链接';

const urlOf = (fixture: KernelFixture, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskPane(page: import('@playwright/test').Page) {
  return page.locator('[data-pane="task-conversation"]');
}

test('Kind 对话运行中：仍是「Kind 工作中」', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ slowMs: 9000 });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText('Kind 工作中', {
      timeout: 30_000,
    });
    await expect(page.getByText('ThinAgent 工作中')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('任务对话运行中：「ThinAgent 工作中」，不出现「Kind 工作中」', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'live' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill(TITLE);
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const card = page.locator('[data-kind="task-result"]').filter({ hasText: CARD_TEXT }).first();
    await expect(card).toHaveCount(1, { timeout: 30_000 });

    // 等任务可读后进任务对话（任务还在跑）。
    let taskId = '';
    for (let attempt = 0; attempt < 600 && !taskId; attempt += 1) {
      const root = JSON.parse(
        await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
      ) as { sessionId: string };
      const text = await readFile(
        path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
        'utf8',
      );
      const match = /已派出任务 ([0-9a-f]+)。/.exec(text);
      if (match) taskId = match[1];
      else await new Promise((resolve) => setTimeout(resolve, 50));
    }
    let readable = false;
    for (let attempt = 0; attempt < 600 && !readable; attempt += 1) {
      const response = await apiPost(request, fixture, { method: 'task-read', taskId });
      if (response.ok()) readable = true;
      else await new Promise((resolve) => setTimeout(resolve, 50));
    }
    expect(readable).toBeTruthy();
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const pane = taskPane(page);
    await expect(pane).toBeVisible({ timeout: 20_000 });
    const process = pane.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText('ThinAgent 工作中', {
      timeout: 30_000,
    });
    await expect(pane.getByText('Kind 工作中')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
