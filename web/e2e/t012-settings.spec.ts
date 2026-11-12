import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T012.9 清单 13：设置页从内置目录加一家 MiniMax（国内站），保存后出现在模型提供商列表。
test('设置页从目录加 MiniMax 连接并保存', async ({ page }) => {
  const fixture = await startKernelFixture({
    vendorModels: ['MiniMax-M3.1-Flash', 'MiniMax-M3'],
  });
  try {
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '添加模型提供商' }).click();
    const add = page.locator('[data-settings="add"]');
    await expect(add).toBeVisible();
    await add.locator('select').selectOption('minimax-cn');
    await add.locator('input[type="password"]').first().fill('sk-cp-e2e-test-key');
    // 把地址改到 e2e 的假端点（真地址在测试里请求不出去）。
    await add.getByText('自定义设置').click();
    await add.locator('input:not([type="password"])').first().fill(fixture.fakeEndpoint);
    await add.getByRole('button', { name: '保存', exact: true }).click();
    await expect(page.getByText('MiniMax（国内站）').first()).toBeVisible({ timeout: 20_000 });
    // 没勾模型就是跟着厂商走：保存时已拉到 2 个未下线的 MiniMax 模型，小标签照厂商顺序列。
    // 页面本地兜底表不能出现厂商已不列的旧 MiniMax-M2.1-highspeed / M2.x。
    await page.getByRole('button', { name: '编辑', exact: true }).last().click();
    await page.locator('[data-settings="editor"]').getByText('自定义设置').click();
    await expect(page.locator('[data-settings="editor"]')).toContainText('跟着厂商的列表走', {
      timeout: 20_000,
    });
    const vendorModels = page.locator('[data-settings="editor"] [data-settings="vendor-model"]');
    await expect(vendorModels).toHaveCount(2);
    await expect(vendorModels.first()).toContainText('MiniMax-M3.1-Flash');
    await expect(vendorModels.nth(1)).toContainText('MiniMax-M3');
    await expect(vendorModels.filter({ hasText: 'MiniMax-M2.1-highspeed' })).toHaveCount(0);
    // 余额/余量常显：打开编辑器自动查（假端点查不到，给「查不到」提示，不报错）。
    await expect(page.locator('[data-settings="quota"]')).toBeVisible();
    await expect(page.locator('[data-settings="quota"]')).toContainText('查不到', { timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

/// T012.9 清单 14：模型失败时失败块给原因和去设置入口，并且不自动换模型。
test('失败块有原因和去设置，模型不变', async ({ page }) => {
  const fixture = await startKernelFixture({ failure: 'credential' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你会失败吗？');
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByText('这一轮没做完', { exact: false })).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: /去设置/ })).toBeVisible();
    // 模型没被自动换：选择器上还是原来的模型。
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('DeepSeek');
  } finally {
    await fixture.close();
  }
});
