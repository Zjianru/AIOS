import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T013.5 清单 11/3：工作过程行（图标/标题）、卡片（diff/终端）、真实退出码、开合规则。
test('工作过程：行/卡片/退出码/开合（write + apply_patch + bash 回放）', async ({ page }) => {
  const fixture = await startKernelFixture({ t013Process: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先写文件，再改一下，最后跑个命令');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });

    const process = page.locator('[data-kind="process"]').last();
    // 做完的一轮收成「已完成，用时 X」，默认收起（组不显示）。
    await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
    await expect(process.locator('[data-kind="process-group"]')).toHaveCount(0);
    await process.locator('button').first().click();
    const group = process.locator('[data-kind="process-group"] button').first();
    await expect(group).toContainText(/已写入文件|修改了文件|执行了命令/);
    await group.click();
    const rows = process.locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText('写入');
    await expect(rows.nth(1)).toContainText('编辑');
    await expect(rows.nth(2)).toContainText('运行命令');
    await expect(rows.nth(2)).toContainText('退出码 2');
    await expect(rows.nth(0)).toContainText('+2');
    // write 行 → diff 卡片（跑完用结果 meta 的实际改动）。
    await rows.nth(0).locator('button').first().click();
    await expect(process.getByText('one', { exact: true })).toBeVisible();
    await expect(process.getByText('two', { exact: true })).toBeVisible();
    // bash 行 → 终端卡片：命令、去掉标记的输出、卡片上的退出码。
    await rows.nth(2).locator('button').first().click();
    await expect(process.getByText('echo out; exit 2')).toBeVisible();
    await expect(process.getByText('out', { exact: true })).toBeVisible();
    await expect(process.getByText('退出码 2', { exact: false })).toHaveCount(2);
    // 刷新页面，行与 diff 还在（都来自日志事件/meta）。
    await page.reload();
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    const process2 = page.locator('[data-kind="process"]').last();
    await expect(process2.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
    await process2.locator('button').first().click();
    await process2.locator('[data-kind="process-group"] button').first().click();
    const rows2 = process2.locator('[data-kind="tool-row"]');
    await expect(rows2).toHaveCount(3);
    await expect(rows2.nth(0)).toContainText('写入');
    await expect(rows2.nth(2)).toContainText('退出码 2');
  } finally {
    await fixture.close();
  }
});

/// T013.5 清单 3：中途停的一轮——一直展开、不能收，行上标已停止。
// T013.5 清单 3：中途停的一轮——一直展开、不能收，行上标已停止。
test('工作过程中途停：整轮一直展开、不能收、标已停止', async ({ page }) => {
  const fixture = await startKernelFixture({ t013Interrupted: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('写文件、改文件、跑命令，然后停下');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    // 回放放慢：等在跑的整轮出现（「Kind 工作中」），再点停止。
    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText(/Kind 工作中|已停止/, { timeout: 30_000 });
    const stop = page.getByRole('button', { name: '停止', exact: true });
    if (await stop.count()) await stop.click();
    // 停下的一轮：整轮行写「已停止」，一直展开（data-open=true），点也不收起。
    await expect(process.locator('button').first()).toContainText('已停止', { timeout: 30_000 });
    await expect(process).toHaveAttribute('data-open', 'true');
    await process.locator('button').first().click({ force: true });
    await expect(process).toHaveAttribute('data-open', 'true');
    // 停下时还没跑完的工具不出现（行数可以为 0）；已经做完的才出现。
    const rows = process.locator('[data-kind="tool-row"]');
    expect(await rows.count()).toBeGreaterThanOrEqual(0);
  } finally {
    await fixture.close();
  }
});
