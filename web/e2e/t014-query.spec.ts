import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const ANSWER = '（T014.2 e2e 回放）这条对话用的模型和花费都查过了，和输入框下面显示的一样。';

async function sendAndWaitAnswer(page: Page): Promise<void> {
  await page.getByPlaceholder('和 Kind 说…').fill('现在用的什么模型，这条对话花了多少？');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(page.locator('[data-kind="assistant-answer"]').getByText(ANSWER)).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, {
    timeout: 60_000,
  });
}

async function openProcess(page: Page): Promise<void> {
  const process = page.locator('[data-kind="process"]').last();
  await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
  await process.locator('button').first().click();
  await process.locator('[data-kind="process-group"] button').first().click();
}

/** 夹具的根会话日志：header 和每条事件的类型。 */
async function sessionLog(fixture: { dataDir: string }): Promise<{
  header: { version: number };
  rows: { type: string; data?: Record<string, unknown> }[];
}> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  const lines = text.trim().split(String.fromCharCode(10)).map((line) => JSON.parse(line));
  return { header: lines[0], rows: lines.slice(1) };
}

/// T014.2 清单 3/4/6（回放）：问模型和花费——工作过程里先加载 skill，再「查 AIOS」；
/// 回答下面没有卡片；会话日志升到 17，加载记录只记一次一条新增。
test('问模型和花费：先加载 skill，再查 AIOS，回答下面没有卡片', async ({ page }) => {
  const fixture = await startKernelFixture({ t014Query: true });
  try {
    await page.goto(urlOf(fixture));
    await sendAndWaitAnswer(page);
    await openProcess(page);

    const rows = page.locator('[data-kind="process"]').last().locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toHaveAttribute('data-tool', 'skill');
    await expect(rows.nth(0)).toContainText('能力');
    await expect(rows.nth(1)).toHaveAttribute('data-tool', 'aios_query');
    await expect(rows.nth(1)).toContainText('查 AIOS');
    await expect(rows.nth(2)).toHaveAttribute('data-tool', 'aios_query');

    // 回答下面没有卡片：这一轮没有提议。
    await expect(page.locator('[data-kind="card-generic"]')).toHaveCount(0);
    await expect(page.locator('[data-kind="card"]')).toHaveCount(0);

    const log = await sessionLog(fixture);
    expect(log.header.version, '会话日志升到 17（只升一次）').toBe(17);
    const registry = log.rows.filter((row) => row.type === 'tool/registry');
    expect(registry.length, '加载记录只在新增时记一条').toBe(2);
    const added = (registry[1].data as { additions: { name: string }[] }).additions.map(
      (tool) => tool.name,
    );
    expect(added).toEqual(['aios_query']);
  } finally {
    await fixture.close();
  }
});

/// T014.2 验收：旧格式（v16）的日志照读——页面照常显示，不报错，也不被改写。
test('旧格式日志照读：16 版写下的对话照常显示', async ({ page }) => {
  const fixture = await startKernelFixture({ t014Query: true, legacySession: true });
  const pageErrors: string[] = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  try {
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="assistant-answer"]')).toContainText(
      '（T014.2 旧日志）16 版写下的回答还在。',
      { timeout: 30_000 },
    );
    expect(pageErrors).toEqual([]);
    const log = await sessionLog(fixture);
    expect(log.header.version, '旧日志照读，不迁移').toBe(16);
  } finally {
    await fixture.close();
  }
});
