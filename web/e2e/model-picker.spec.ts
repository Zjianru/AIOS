import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

test('底栏选模型：下一轮请求用新模型', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    // T012.9：给连接补第二行模型（底栏只列这个连接里存着的模型）。
    await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: fixture.fakeEndpoint,
      model: 'deepseek-flash',
      models: [{ id: 'deepseek-flash' }, { id: 'deepseek-v4-pro' }],
    });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.locator('[data-kind="model-picker"]').click();
    await page.getByRole('menuitem', { name: /DeepSeek-V4-Pro/ }).click({ force: true });
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect.poll(() => fixture.lastModel(), { timeout: 30_000 }).toBe('deepseek-pro');
  } finally {
    await fixture.close();
  }
});

/// T008 r4 #3：换模型等成功再改状态；失败给中文原因，不吞。
test('换模型失败：不先改状态，给中文原因', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: fixture.fakeEndpoint,
      model: 'deepseek-flash',
      models: [{ id: 'deepseek-flash' }, { id: 'deepseek-v4-pro' }],
    });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.locator('[data-kind="model-picker"]').click();
    await page.route(/\/api(\?|$)/, async (route, request) => {
      if ((request.postData() ?? '').includes('session-model-select')) {
        await route.fulfill({ status: 500, contentType: 'application/json', body: '{}' });
        return;
      }
      await route.continue();
    });
    await page.getByRole('menuitem', { name: /DeepSeek-V4-Pro/ }).click({ force: true });
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('DeepSeek Chat');
    await expect(page.getByText('换模型没成：', { exact: false })).toBeVisible({ timeout: 15_000 });
  } finally {
    await fixture.close();
  }
});

/// T008 r4 #3：日志没记本轮模型，每轮不再显示模型名（底部选择器的初始值另见 T012）。
test('每轮不显示模型名', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText('DeepSeek Chat · 输入')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
