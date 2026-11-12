import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T038.2：任务对话页顶栏是「任务对话 › 任务标题」，不出现原始 task id；点顶栏回 Kind。
 * 页内「回 Kind 对话」保留不改。
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

/** 根会话日志里 task_accept 回执带的任务 id（等 Kind 真的派出任务）。 */
async function taskIdOf(fixture: KernelFixture): Promise<string> {
  for (let attempt = 0; attempt < 600; attempt += 1) {
    const match = /已派出任务 ([0-9a-f]+)。/.exec(await rootLogText(fixture));
    if (match) return match[1];
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('等不到 Kind 派出任务');
}

test('任务对话顶栏显示任务标题、可点回 Kind，不出现 task id', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('把 hello.txt 的判重改成按 url');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const taskId = await taskIdOf(fixture);

    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({
      timeout: 30_000,
    });
    const toolbar = page.locator('header').first();
    // 顶栏：有任务标题、没有原始 id。
    await expect(toolbar).toContainText(TITLE, { timeout: 30_000 });
    await expect(toolbar).not.toContainText(taskId);
    // 点顶栏回 Kind 主页面。
    await page.locator('[data-kind="toolbar-back"]').click();
    await expect(page).toHaveURL(/#kind/);
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible();
  } finally {
    await fixture.close();
  }
});
