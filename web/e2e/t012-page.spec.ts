import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** T012 F1：四类单价都填上（网络目录可能没有缓存价，用户价保证有价格可算）。 */
async function setUserPrices(
  request: import('@playwright/test').APIRequestContext,
  fixture: { fakeEndpoint: string },
) {
  const response = await apiPost(request, fixture as never, {
    method: 'connection-update',
    id: 'deepseek',
    endpoint: fixture.fakeEndpoint,
    model: 'deepseek-flash',
    models: [
      {
        id: 'deepseek-flash',
        priceInput: 0.15,
        priceOutput: 0.6,
        priceCacheRead: 0.003,
        priceCacheWrite: 0.1,
      },
    ],
  });
  expect(response.ok()).toBeTruthy();
}

/// T012.9 清单 13：输入框下的统计行——轮/步、输入输出、缓存命中、花费（有价格才显示）。
test('统计行：轮/步/输入输出/缓存命中/花费', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageReadOnly: true });
  try {
    await setUserPrices(request, fixture);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
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

/// T012.9 清单 13：换思考档——选 low（默认 high 不再点也过），断言下一轮真的按 low 序列化。
test('换思考档：下一轮请求的 thinking 档按选择', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    // T012.9 底栏标签是异步 hydration 的（模型名 + 思考档）；等它出来再点菜单。
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('·', { timeout: 30_000 });
    await page.locator('[data-kind="model-picker"]').click();
    const effort = page.locator('[data-kind="effort"]');
    await expect(effort).toBeVisible({ timeout: 30_000 });
    await effort.getByRole('button', { name: 'low', exact: true }).click({ force: true });
    await page.waitForTimeout(500);
    await page.keyboard.press('Escape');
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    const request = await fixture.lastRequest();
    expect(request?.model).toBe('deepseek-flash');
    // DeepSeek 的 low = thinking enabled / budget 4096（T012.3 档位表）。
    expect(request?.thinking).toMatchObject({ type: 'enabled', budget_tokens: 4096 });
  } finally {
    await fixture.close();
  }
});

/// T012.9 故事 3：换到另一个模型后，档位选项立刻跟着这个模型走（不等下一轮）。
test('换模型后档位跟着变', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: fixture.fakeEndpoint,
      model: 'deepseek-flash',
      models: [{ id: 'deepseek-flash' }, { id: 'deepseek-v4-pro' }],
    });
    await page.goto(urlOf(fixture));
    // 第 1 轮：默认 flash。
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    // 用服务接口换到 pro（下一轮生效），再发一轮触发统计/模型刷新。
    const selected = await apiPost(request, fixture, {
      method: 'session-model-select',
      connection: 'deepseek',
      model: 'deepseek-v4-pro',
    });
    expect(selected.ok()).toBeTruthy();
    await page.getByPlaceholder('和 Kind 说…').fill('第二轮');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    // 换模型后档位选项立刻按这个模型出现，且能切（序列化由上面「换思考档」覆盖）。
    await page.locator('[data-kind="model-picker"]').click();
    const effort = page.locator('[data-kind="effort"]');
    await expect(effort).toBeVisible({ timeout: 20_000 });
    await expect(effort.getByRole('button', { name: 'max', exact: true })).toBeVisible();
    await effort.getByRole('button', { name: 'low', exact: true }).click({ force: true });
    await expect(page.locator('[data-kind="effort-now"]')).toContainText('弱', { timeout: 10_000 });
    await page.keyboard.press('Escape');
    // 上一轮请求已经用了新模型（下一轮生效的语义）。
    expect(await fixture.lastModel()).toBe('deepseek-v4-pro');
  } finally {
    await fixture.close();
  }
});

/// T012.9 清单 13：币种切换——走设置页的「显示币种」按钮，回 Kind 后花费按人民币显示。
test('币种：设置页切 CNY 后花费显示 ¥', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageReadOnly: true });
  try {
    await setUserPrices(request, fixture);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    await expect(page.locator('[data-kind="stats-cost"]')).toContainText('$');
    // 真实 UI：设置页 › 显示币种 › ¥ CNY。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const currency = page.locator('[data-settings="currency"]');
    await expect(currency).toBeVisible({ timeout: 20_000 });
    await currency.getByRole('button', { name: '¥ CNY' }).click();
    // 回 Kind，统计行的花费变人民币。
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="stats-cost"]')).toContainText('¥', { timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

/// T012 F1（Claude 审核）：设置页只填缓存读价，合计按四类单价变（原来缓存读也乘输入价，高约 10 倍）。
test('只填缓存读价：合计变了', async ({ page }) => {
  const fixture = await startKernelFixture({ usageReadOnly: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    const cost = page.locator('[data-kind="stats-cost"]');
    await expect(cost).toBeVisible({ timeout: 20_000 });
    const before = (await cost.innerText()).trim();
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await page.locator('[data-settings="model-price"]').first().click();
    const priceEditor = page.locator('[data-settings="price"]');
    await expect(priceEditor).toBeVisible();
    await expect(priceEditor.locator('[data-settings="price-priceInput"]')).toBeVisible();
    await expect(priceEditor.locator('[data-settings="price-priceCacheRead"]')).toBeVisible();
    await expect(priceEditor.locator('[data-settings="price-priceCacheWrite"]')).toBeVisible();
    await expect(priceEditor.locator('[data-settings="price-priceOutput"]')).toBeVisible();
    await priceEditor.locator('[data-settings="price-priceCacheRead"]').fill('100');
    await editor.getByRole('button', { name: '保存', exact: true }).click();
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 20_000 });
    await page.reload();
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });
    await expect(cost).toBeVisible({ timeout: 20_000 });
    await expect.poll(async () => (await cost.innerText()).trim(), { timeout: 20_000 }).not.toBe(before);
  } finally {
    await fixture.close();
  }
});

/// T012 F2（Claude 审核）：换到没有这一档的模型——选择器显示实际档，提示写明「没有」；换回来恢复原来要的档。
test('换到没有这一档的模型：显示最近档并告诉用户', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    const saved = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M3.1-Flash',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'sk-dod-test-key',
      preset: 'minimax-cn',
      models: [{ id: 'MiniMax-M3.1-Flash' }],
    });
    expect(saved.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });
    // T012.9 底栏标签是异步 hydration 的（模型名 + 思考档「…·强」）；等它出来再点菜单。
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('·', { timeout: 30_000 });
    await page.locator('[data-kind="model-picker"]').click();
    const deepseekItem = page.getByRole('menuitem', { name: /DeepSeek/ }).first();
    await expect(deepseekItem).toBeVisible({ timeout: 30_000 });
    await deepseekItem.click({ force: true });
    await page.waitForTimeout(500);
    await page.locator('[data-kind="model-picker"]').click();
    const effort = page.locator('[data-kind="effort"]');
    await expect(effort).toBeVisible({ timeout: 20_000 });
    await effort.getByRole('button', { name: 'none', exact: true }).click({ force: true });
    await expect(page.locator('[data-kind="effort-now"]')).toContainText('不思考', { timeout: 20_000 });
    await page.keyboard.press('Escape');

    await page.locator('[data-kind="model-picker"]').click();
    const minimaxItem = page.getByRole('menuitem', { name: /MiniMax-M3\.1-Flash/ });
    await expect(minimaxItem).toBeVisible({ timeout: 30_000 });
    await minimaxItem.click({ force: true });
    await expect(page.locator('[data-kind="model-picker"]')).toContainText('MiniMax-M3.1-Flash', {
      timeout: 20_000,
    });
    const notice = page.getByText('没有「不思考」，按「弱」', { exact: false });
    if (!(await notice.isVisible().catch(() => false))) {
      await page.locator('[data-kind="model-picker"]').click();
      await page.getByRole('menuitem', { name: /MiniMax-M3\.1-Flash/ }).click({ force: true });
    }
    await expect(notice).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="effort-now"]')).toContainText('弱', { timeout: 20_000 });

    await page.locator('[data-kind="model-picker"]').click();
    await page.getByRole('menuitem', { name: /DeepSeek/ }).first().click({ force: true });
    await expect(page.locator('[data-kind="effort-now"]')).toContainText('不思考', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
