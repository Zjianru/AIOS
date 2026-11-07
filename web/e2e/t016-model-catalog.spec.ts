import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// 设置页「获取可用模型」显示目录窗口；把目录里的新模型加进来时，
/// 四类价格（输入/输出/缓存读/缓存写）跟着列表走，不丢。
test('设置页获取模型：目录窗口和四类价格都在', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    // 目录窗口来自 available-models（camelCase）；deepseek-flash 是 1048576（页面写成 M）。
    await expect(discover.locator('label', { hasText: 'deepseek-flash' })).toContainText('1.048576M');
    // 把目录里多出来的模型勾上：它带着窗口和四类价格进模型列表。
    await discover
      .locator('label', { hasText: 'deepseek-v4-pro' })
      .locator('input[type="checkbox"]')
      .check();
    await editor.getByRole('button', { name: '更新列表' }).click();
    await expect(discover).toHaveCount(0);
    const prices = editor.locator('[data-settings="model-price"]');
    await expect(prices).toHaveCount(2);
    await expect(prices.nth(1)).toContainText('in 0.66');
    await expect(prices.nth(1)).toContainText('out 1.98');
    await expect(prices.nth(1)).toContainText('r 0.022');
    await expect(editor.locator('input[value="1.048576M"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});

/** 假厂商的 /v1/models：e2e 要看厂商真正回的列表，不用回放脚本那张 404。 */
async function startVendor(models: string[]): Promise<{ url: string; close: () => Promise<void> }> {
  const server = createServer((request, response) => {
    if (request.url?.includes('/v1/models')) {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ data: models.map((id) => ({ id, display_name: id })) }));
      return;
    }
    response.writeHead(404, { 'content-type': 'application/json' });
    response.end('{}');
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve());
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('厂商 mock 没拿到端口');
  return {
    url: `http://127.0.0.1:${address.port}`,
    close: () => new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))),
  };
}

/// 获取面板：「新」、搜索、计数、不勾就跟着厂商走；选择器和小标签用厂商列表。
test('获取模型标新、搜索计数，不勾就跟着厂商走', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const vendor = await startVendor(['deepseek-flash', 'vendor-future']);
  try {
    const updated = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: vendor.url,
      model: 'deepseek-flash',
    });
    expect(updated.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    let editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });

    // 计数：目录 + 厂商合并列出 5 个；目录里没有的 vendor-future 是新的。
    const count = discover.locator('[data-settings="discover-count"]');
    await expect(count).toContainText('列出 5 个');
    await expect(count).toContainText('1 个是新的');
    const future = discover.locator('label', { hasText: 'vendor-future' });
    await expect(future.locator('[data-settings="fresh"]')).toHaveText('新');
    await expect(
      discover.locator('label', { hasText: 'deepseek-flash' }).locator('[data-settings="fresh"]'),
    ).toHaveCount(0);

    // 搜索只剩匹配行。
    const search = discover.getByPlaceholder('搜索模型');
    await search.fill('future');
    await expect(discover.locator('label')).toHaveCount(1);
    await expect(discover.locator('label')).toContainText('vendor-future');
    await search.fill('');

    // 一个都不勾：提示跟着厂商走，更新后列表为空、小标签列出厂商模型。
    await discover.getByRole('button', { name: '取消全选' }).click();
    await expect(count).toContainText('一个没勾');
    await expect(discover).toContainText('一个都不勾：跟着厂商的列表走');
    await discover.getByRole('button', { name: '更新列表' }).click();
    await expect(discover).toHaveCount(0);
    await expect(editor).toContainText('跟着厂商的列表走');
    await expect(editor.locator('[data-settings="vendor-model"]')).toHaveCount(2);

    // 保存后重开：空表（跟厂商走）和厂商小标签都还在。
    await editor.getByRole('button', { name: '保存', exact: true }).click();
    await expect(page.locator('[data-settings="editor"]')).toHaveCount(0);
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await expect(editor).toContainText('跟着厂商的列表走');
    await expect(editor.locator('[data-settings="vendor-model"]')).toHaveCount(2);

    // 选择器跟可选的模型（厂商列表），不再列目录独有项。
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.locator('[data-kind="model-picker"]').click();
    const futureItem = page.getByRole('menuitem', { name: /vendor-future/ });
    await expect(futureItem).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole('menuitem', { name: /deepseek-v4-pro/ })).toHaveCount(0);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});

/// 勾两个就只用勾的：选择器里只有勾的，没有厂商其它模型、也没有目录独有项。
test('勾两个模型，选择器里只有这两个', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const vendor = await startVendor(['deepseek-flash', 'vendor-future', 'vendor-extra']);
  try {
    const updated = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: vendor.url,
      model: 'deepseek-flash',
    });
    expect(updated.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    // deepseek-flash 已勾；再勾 vendor-extra，vendor-future 不勾。
    await discover
      .locator('label', { hasText: 'vendor-extra' })
      .locator('input[type="checkbox"]')
      .check();
    await discover.getByRole('button', { name: '更新列表' }).click();
    await expect(discover).toHaveCount(0);
    await expect(editor).toContainText('只用勾的 2 个');
    await editor.getByRole('button', { name: '保存', exact: true }).click();
    await expect(page.locator('[data-settings="editor"]')).toHaveCount(0);

    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.locator('[data-kind="model-picker"]').click();
    await expect(page.getByRole('menuitem', { name: /vendor-extra/ })).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole('menuitem', { name: /DeepSeek-V4.1-Flash/ })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: /vendor-future/ })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: /deepseek-v4-pro/ })).toHaveCount(0);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});
