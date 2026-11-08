import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const ANSWER = '（T014.1 e2e 回放）两个文件都写好了。';

async function sendAndOpenProcess(page: Page): Promise<void> {
  await page.getByPlaceholder('和 Kind 说…').fill('写两个文件');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(page.locator('[data-kind="assistant-answer"]').getByText(ANSWER)).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, {
    timeout: 60_000,
  });
  const process = page.locator('[data-kind="process"]').last();
  await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
  await process.locator('button').first().click();
  await process.locator('[data-kind="process-group"] button').first().click();
}

/// T014.1 清单 17：工作过程里的路径——工作目录里的显示相对路径，工作目录外的原样。
test('工作过程路径：工作目录内相对，工作目录外原样', async ({ page }) => {
  const fixture = await startKernelFixture({ pathsTurn: true });
  const paths = fixture.paths;
  if (!paths) throw new Error('pathsTurn 没给 paths');
  try {
    await page.goto(urlOf(fixture));
    await sendAndOpenProcess(page);
    const rows = page.locator('[data-kind="process"]').last().locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0).locator('.process-path')).toHaveText('hello.txt');
    await expect(rows.nth(1).locator('.process-path')).toHaveText(paths.outside + '/outside.txt');
    // 刷新后还是从日志算出来的一样（cwd 重新从 status 拿）。
    await page.reload();
    await expect(page.locator('[data-kind="assistant-answer"]').getByText(ANSWER)).toBeVisible({
      timeout: 30_000,
    });
    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    const rows2 = process.locator('[data-kind="tool-row"]');
    await expect(rows2).toHaveCount(2);
    await expect(rows2.nth(0).locator('.process-path')).toHaveText('hello.txt');
    await expect(rows2.nth(1).locator('.process-path')).toHaveText(paths.outside + '/outside.txt');
  } finally {
    await fixture.close();
  }
});

/// T014.1 验收：旧的状态消息没有 cwd——路径照原样显示，不报错。
test('旧的状态消息没有 cwd：路径原样显示，页面不报错', async ({ page }) => {
  const fixture = await startKernelFixture({ pathsTurn: true });
  const paths = fixture.paths;
  if (!paths) throw new Error('pathsTurn 没给 paths');
  const pageErrors: string[] = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.route(/\/api(\?|$)/, async (route) => {
    if (!(route.request().postData() ?? '').includes('"method":"status"')) {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const json = (await response.json()) as Record<string, unknown>;
    delete json.cwd;
    await route.fulfill({ response, json });
  });
  try {
    await page.goto(urlOf(fixture));
    await sendAndOpenProcess(page);
    const rows = page.locator('[data-kind="process"]').last().locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0).locator('.process-path')).toHaveText(paths.cwd + '/hello.txt');
    await expect(rows.nth(1).locator('.process-path')).toHaveText(paths.outside + '/outside.txt');
    expect(pageErrors).toEqual([]);
  } finally {
    await fixture.close();
  }
});
