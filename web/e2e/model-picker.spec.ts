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
    await expect.poll(() => fixture.lastModel(), { timeout: 30_000 }).toBe('deepseek-v4-pro');
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
    // T012.9 的底栏标签是异步 hydration 的（模型名 + 思考档）：先等它出来（菜单项才有显示名）。
    const picker = page.locator('[data-kind="model-picker"]');
    await page.locator('[data-kind="model-picker"]').click();
    await expect(picker).toContainText(/Flash/, { timeout: 30_000 });
    await page.route(/\/api(\?|$)/, async (route, request) => {
      if ((request.postData() ?? '').includes('session-model-select')) {
        await route.fulfill({ status: 500, contentType: 'application/json', body: '{}' });
        return;
      }
      await route.continue();
    });
    // 菜单项是异步 hydration 出来的：等它真的可见再点（并发/慢机器上会晚）。
    const item = page.getByRole('menuitem', { name: /DeepSeek-V4-Pro/ });
    await expect(item).toBeVisible({ timeout: 30_000 });
    await item.click({ force: true });
    await expect(picker).not.toContainText('V4-Pro');
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
    // 每轮不显示模型名与用量（用量只在悬停底栏那行时出现；DOM 里有隐藏的 hover 行，要看可见的）。
    await expect(page.getByText(/· 输入 \d+/).filter({ visible: true })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
