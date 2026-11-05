import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T012.9 清单 13：输入框下的统计行——轮/步、输入输出、缓存命中、花费（有价格才显示）。
test('统计行：轮/步/输入输出/缓存命中/花费', async ({ page }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    const dock = page.locator('[data-kind="composer-dock"]');
    await expect(dock).toBeVisible();
    await expect(dock).toContainText('轮');
    await expect(dock).toContainText('步');
    // 缓存命中来自 usage 夹具（缓存读 200 / 全部输入 1250 → 16%）。
    await expect(dock).toContainText('缓存命中');
    // 花费：模型目录里有 DeepSeek 价格 → 显示美元金额（不是空）。
    await expect(page.locator('[data-kind="stats-cost"]')).toContainText('$');
  } finally {
    await fixture.close();
  }
});

/// T012.9 清单 13：换思考档——下一轮请求带这个模型自己的档（thinking enabled）。
test('换思考档：下一轮请求的 thinking 档按选择', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.locator('[data-kind="model-picker"]').click();
    const effort = page.locator('[data-kind="effort"]');
    await expect(effort).toBeVisible();
    await effort.getByRole('button', { name: 'high', exact: true }).click({ force: true });
    await page.keyboard.press('Escape');
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    const request = await fixture.lastRequest();
    expect(request?.model).toBe('deepseek-flash');
    expect(request?.thinking).toMatchObject({ type: 'enabled' });
  } finally {
    await fixture.close();
  }
});

/// T012.9 清单 13：币种切换——设置里切到 CNY 后，花费按人民币显示。
test('币种：设置切 CNY 后花费显示 ¥', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    const response = await apiPost(request, fixture, { method: 'settings-update', currency: 'cny' });
    expect(response.ok()).toBeTruthy();
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="stats-cost"]')).toContainText('¥');
  } finally {
    await fixture.close();
  }
});
