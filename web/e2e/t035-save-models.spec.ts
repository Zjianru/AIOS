import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { apiPost, startKernelFixture } from './service-fixture';

/**
 * T035.2：连接验证整条删掉，改由「保存连接时拉一次厂商列表」写 check。
 * 这里用假厂商看页面能看到的部分：行上不再有「验证」按钮、状态写列表结果、
 * 已下线的模型不进「获取」面板。保存的实现在 service 接缝测过，这里只验页面。
 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** 假厂商：/v1/models 回给定列表，或回一个固定状态码。 */
async function startVendor(options: { models?: string[]; status?: number }): Promise<{
  url: string;
  modelsRequests: () => number;
  close: () => Promise<void>;
}> {
  let modelsRequests = 0;
  const server = createServer((request, response) => {
    if (request.url?.includes('/v1/models')) {
      modelsRequests += 1;
      if (options.status && options.status !== 200) {
        response.writeHead(options.status, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ error: { message: 'invalid api key' } }));
        return;
      }
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          data: (options.models ?? []).map((id) => ({ id, display_name: id })),
        }),
      );
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
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  };
}

test('保存连接拉一次厂商列表：没有验证按钮，获取面板不列已下线模型', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  // MiniMax 的 /v1/models 仍会给已下线的 M2.7；目录标了 deprecated，页面不列。
  const vendor = await startVendor({
    models: ['MiniMax-M3.1-Flash', 'MiniMax-M3', 'MiniMax-M2.7'],
  });
  try {
    const saved = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M3.1-Flash',
      endpoint: vendor.url,
      apiKey: 'sk-e2e-t035',
      preset: 'minimax-cn',
      models: [],
    });
    expect(saved.ok()).toBeTruthy();
    // 保存这一条请求里就拉了一次厂商列表。
    expect(vendor.modelsRequests()).toBe(1);

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    // 连接行上找不到「验证」按钮。
    await expect(page.getByRole('button', { name: '验证', exact: true })).toHaveCount(0);
    // 状态一句话写「已保存」和「模型列表」两件事。
    await expect(page.getByText(/已保存在 Keyring（/).first()).toBeVisible();
    await expect(page.getByText(/已拿到模型列表（/)).toBeVisible({ timeout: 20_000 });

    // 「获取」面板：厂商列表照旧问一次，但目录标了 deprecated 的 M2.7 不列。
    await page.getByRole('button', { name: '编辑', exact: true }).last().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    await expect(discover.locator('label')).toHaveCount(2);
    await expect(discover).not.toContainText('MiniMax-M2.7');
    await expect(discover).toContainText('MiniMax-M3.1-Flash');
    expect(vendor.modelsRequests()).toBe(2);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});

test('保存时厂商回 401：保存照样成功，行上写密钥不对', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const vendor = await startVendor({ status: 401 });
  try {
    const saved = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'bad-key',
      name: 'Bad key',
      model: 'MiniMax-M3.1-Flash',
      endpoint: vendor.url,
      apiKey: 'sk-e2e-t035-bad',
      preset: 'minimax-cn',
      models: [],
    });
    expect(saved.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText(/拉不到模型列表：密钥不对（401）/)).toBeVisible({ timeout: 20_000 });
    // 拉不到列表不挡聊天：模型选择器里照样能选这个连接。
    await expect(page.getByRole('button', { name: '验证', exact: true })).toHaveCount(0);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});
