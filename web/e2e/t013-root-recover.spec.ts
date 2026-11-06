import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/// T013.9 修法 A/C：空库 → 设置页加连接（假端点，保存时服务端自动验证）→ 回 Kind 点「开始」→
/// recheck 起 root（修法 A 让服务端 factory 能解析到刚保存的连接）→ Kind 有 composer、能发消息。
test('空库加连接后 root 恢复：Kind 有 composer 能发消息', async ({ page }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    // 空库：先显示首次使用页（侧栏也能进设置）。
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 30_000 });
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 30_000 });
    const add = page.locator('[data-settings="add"]');
    if ((await add.count()) === 0) {
      await page.getByRole('button', { name: '添加模型提供商' }).click();
    }
    await expect(add).toBeVisible({ timeout: 20_000 });
    await add.locator('select').selectOption('deepseek');
    await add.locator('input[type="password"]').first().fill('sk-test-root-recover');
    // 地址改到 e2e 假端点（保存时服务端会发最小请求验证，假端点回 200）。
    await add.getByText('自定义设置').click();
    await add.locator('input:not([type="password"])').first().fill(fixture.fakeEndpoint);
    await add.getByRole('button', { name: '保存', exact: true }).click();
    await expect(page.getByText('DeepSeek').first()).toBeVisible({ timeout: 30_000 });
    // 回 Kind：空库 firstUse 还在 → 点「开始」触发 recheck。
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.waitForTimeout(2000);
    const start = page.getByRole('button', { name: '开始', exact: true });
    if ((await start.count()) > 0) {
      await start.waitFor({ state: 'visible', timeout: 15_000 });
      await start.click();
    }
    // 修法 C：onConnected → recheck；修法 A：factory 能解析到刚保存的连接 → root 恢复。
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
    await page.locator('[data-kind="composer-input"]').fill('你好');
    await page.locator('[data-kind="send"]').click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
