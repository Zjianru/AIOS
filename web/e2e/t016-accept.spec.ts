import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { apiPost, startKernelFixture } from './service-fixture';

/// 验收修复的页面行为：获取面板按厂商给的顺序原样列出（第一次获取一条都不标新）、
/// 价格摘要紧凑、套餐余量常显并自动查询。这里只做页面能看到的行为。
const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** 假厂商：/v1/models 回列表；/user/balance 回余额（可延迟，用来看转圈）。 */
async function startVendor(options: { models?: string[]; balanceDelayMs?: number } = {}): Promise<{
  url: string;
  modelsRequests: () => number;
  balanceRequests: () => number;
  close: () => Promise<void>;
}> {
  const models = options.models ?? [];
  const balanceDelayMs = options.balanceDelayMs ?? 0;
  let modelsRequests = 0;
  let balanceRequests = 0;
  const server = createServer((request, response) => {
    if (request.url?.includes('/v1/models')) {
      modelsRequests += 1;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ data: models.map((id) => ({ id, display_name: id })) }));
      return;
    }
    if (request.url?.includes('/user/balance')) {
      balanceRequests += 1;
      const reply = () => {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(
          JSON.stringify({ balance_infos: [{ currency: 'CNY', total_balance: '110.00' }] }),
        );
      };
      if (balanceDelayMs > 0) setTimeout(reply, balanceDelayMs);
      else reply();
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
    modelsRequests: () => modelsRequests,
    balanceRequests: () => balanceRequests,
    close: () => new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve()))),
  };
}

/// 获取面板按厂商给的顺序原样列出，但目录标了 deprecated 的旧 MiniMax（厂商 /v1/models
/// 仍会给）不列；第一次获取没有基准，一条都不标新。
test('获取面板按厂商顺序列出未下线模型，第一次不标新', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const vendor = await startVendor({
    models: [
      'MiniMax-M3.1-Flash',
      'MiniMax-M3',
      'MiniMax-M2.7',
      'MiniMax-M2.7-highspeed',
      'MiniMax-M2.5',
      'MiniMax-M2.5-highspeed',
      'MiniMax-M2.1',
      'MiniMax-M2.1-highspeed',
      'MiniMax-M2',
    ],
  });
  try {
    const saved = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M3.1-Flash',
      endpoint: vendor.url,
      apiKey: 'sk-e2e-minimax',
      preset: 'minimax-cn',
      models: [],
    });
    expect(saved.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).last().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });

    const labels = discover.locator('label');
    // M2.1-highspeed 不在 models.dev 目录里（目录没有的保留）；目录标了 deprecated 的六条去掉。
    await expect(labels).toHaveCount(3);
    const order = ['MiniMax-M3.1-Flash', 'MiniMax-M3', 'MiniMax-M2.1-highspeed'];
    for (const [index, id] of order.entries()) {
      await expect(labels.nth(index)).toContainText(id);
    }
    // 第一次获取（这个连接还没有存下的列表）：一个都不标「新」。
    await expect(discover.locator('[data-settings="fresh"]')).toHaveCount(0);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});

/// 验收发现 1（纠偏）：列表不重排——厂商怎么排就怎么显示，不按版本重排。
test('获取模型列表跟厂商顺序，不按版本重排', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  // 故意不按版本/发布日期排：M3 在前、M3.1-Flash 在后；按版本排会变成 M3.1/M3。
  const vendor = await startVendor({ models: ['MiniMax-M3', 'MiniMax-M3.1-Flash'] });
  try {
    const saved = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M3.1-Flash',
      endpoint: vendor.url,
      apiKey: 'sk-e2e-minimax-order',
      preset: 'minimax-cn',
      models: [],
    });
    expect(saved.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).last().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });

    const labels = discover.locator('label');
    await expect(labels).toHaveCount(2);
    await expect(labels.nth(0)).toContainText('MiniMax-M3');
    await expect(labels.nth(1)).toContainText('MiniMax-M3.1-Flash');
  } finally {
    await vendor.close();
    await fixture.close();
  }
});

/// 验收发现 3：余额/套餐余量不再折叠；打开编辑器自动查一次，手动刷新立即查并转圈，
/// 同一连接在缓存/最小间隔内再打开不重复打厂商接口。
test('套餐余量常显：打开自动查一次，刷新立即查并转圈', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const vendor = await startVendor({ balanceDelayMs: 600 });
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

    // 不点折叠、不点查询：一打开编辑器就自动查，查询中可见转圈。
    const quota = editor.locator('[data-settings="quota"]');
    await expect(quota).toBeVisible();
    await expect(quota.locator('[data-settings="quota-spinner"]')).toBeVisible();
    await expect(quota).toContainText('CNY 110.00', { timeout: 10_000 });
    expect(vendor.balanceRequests()).toBe(1);

    // 手动刷新：立即查（不等缓存），同样转圈。
    await quota.getByRole('button', { name: '刷新' }).click();
    await expect(quota.locator('[data-settings="quota-spinner"]')).toBeVisible();
    await expect.poll(() => vendor.balanceRequests(), { timeout: 10_000 }).toBe(2);
    await expect(quota).toContainText('CNY 110.00', { timeout: 10_000 });

    // 收起再打开：缓存/最小间隔内不重复查。
    await page.getByRole('button', { name: '收起', exact: true }).click();
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    await expect(editor).toBeVisible();
    await expect(editor.locator('[data-settings="quota"]')).toContainText('CNY 110.00');
    await page.waitForTimeout(400);
    expect(vendor.balanceRequests()).toBe(2);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});
