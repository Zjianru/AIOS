import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T012.9 清单 13：设置页从内置目录加一家 MiniMax（国内站），保存后出现在模型提供商列表。
test('设置页从目录加 MiniMax 连接并保存', async ({ page }) => {
  const fixture = await startKernelFixture();
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
    // 设置里也显示模型行（MiniMax M3.1 Flash 来自内置目录）；先展开自定义设置。
    await page.getByRole('button', { name: '编辑', exact: true }).last().click();
    await page.locator('[data-settings="editor"]').getByText('自定义设置').click();
    await expect(page.locator('[data-settings="editor"]')).toContainText('4 个模型', {
      timeout: 20_000,
    });
    await expect(
      page.locator('[data-settings="editor"] input[value="MiniMax-M3.1-Flash"]'),
    ).toHaveCount(2); // 模型 ID 和显示名称两个输入都是这个值
    // 余额/余量查询入口在（假端点查不到，给「查不到」提示，不报错）。
    await page.getByText('余额与套餐余量').click();
    await page.getByRole('button', { name: '查询余额/余量' }).click();
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
