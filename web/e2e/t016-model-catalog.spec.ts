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
    // 紧凑：只显示输入/输出两格；四类完整值和「标价=目录价」在 title。
    await expect(prices.nth(0)).toHaveText('标价');
    await expect(prices.nth(1)).toHaveText('$0.66 / $1.98');
    const priceTitle = await prices.nth(1).getAttribute('title');
    expect(priceTitle).toContain('输入 $0.66');
    expect(priceTitle).toContain('输出 $1.98');
    expect(priceTitle).toContain('缓存读 $0.022');
    expect(priceTitle).toContain('缓存写');
    expect(priceTitle).toContain('不乘倍率');
    await expect(editor.locator('[data-settings="price-help"]')).toContainText('标价 = 目录价');
    await expect(editor.locator('[data-settings="price-help"]')).toContainText('不乘倍率');
    await expect(editor.locator('input[value="1.048576M"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});

/** 假厂商的 /v1/models：e2e 要看厂商真正回的列表，不用回放脚本那张 404。 */
async function startVendor(models: string[]): Promise<{
  url: string;
  /** 收到过几次 /v1/models：用来确认「页面/选择器确实问过厂商」。 */
  modelsRequests: () => number;
  close: () => Promise<void>;
}> {
  let modelsRequests = 0;
  const server = createServer((request, response) => {
    if (request.url?.includes('/v1/models')) {
      modelsRequests += 1;
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
    modelsRequests: () => modelsRequests,
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

/// 打开页面/选择器会顺带问一次厂商（补价格、窗口和档），但只有按「获取可用模型」才把
/// 这一次的列表存下来。隐式查询也落盘的话，「新」的基准会被推着走，设置页就再也看不到「新」。
test('隐式查询不存厂商列表，按获取才存下并标新', async ({ page, request }) => {
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
    await page.locator('[data-kind="model-picker"]').click();
    // 首屏和选择器确实问过厂商（补价格/窗口/档，隐式那条路走了），
    // 但设置页还不该有这家存下的列表。
    await expect.poll(() => vendor.modelsRequests(), { timeout: 20_000 }).toBeGreaterThan(0);
    const before = await apiPost(request, fixture, { method: 'model-settings' });
    expect((await before.json()).listed).not.toHaveProperty('deepseek');

    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    // 基准还是内置目录：厂商多出来的那个才算新。
    await expect(discover.locator('[data-settings="discover-count"]')).toContainText('1 个是新的');
    await expect(
      discover.locator('label', { hasText: 'vendor-future' }).locator('[data-settings="fresh"]'),
    ).toHaveText('新');

    // 按过「获取」以后，这一次的列表才存进设置页的 listed。
    const after = await apiPost(request, fixture, { method: 'model-settings' });
    const listed = (await after.json()).listed as Record<string, { models: { id: string }[] }>;
    expect(listed.deepseek.models.map((row) => row.id)).toEqual(['deepseek-flash', 'vendor-future']);
  } finally {
    await vendor.close();
    await fixture.close();
  }
});

/// 换了 API 地址，旧地址存下的厂商列表就不能再算数：小标签和主模型要回到内置目录，
/// 直到在新地址按一次「获取可用模型」。
test('换 API 地址后旧厂商列表作废，获取后新表生效', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const first = await startVendor(['deepseek-flash', 'vendor-old']);
  const second = await startVendor(['deepseek-flash', 'vendor-new']);
  const listedIds = async () => {
    const response = await apiPost(request, fixture, { method: 'model-settings' });
    const listed = (await response.json()).listed as Record<string, { models: { id: string }[] }>;
    return listed.deepseek?.models.map((row) => row.id) ?? [];
  };
  try {
    const toFirst = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: first.url,
      model: 'deepseek-flash',
    });
    expect(toFirst.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    await expect(editor.locator('[data-settings="discover"]')).toBeVisible({ timeout: 20_000 });
    expect(await listedIds()).toEqual(['deepseek-flash', 'vendor-old']);

    // 换到另一个地址：旧地址的那份列表马上失效。
    const toSecond = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: second.url,
      model: 'deepseek-flash',
    });
    expect(toSecond.ok()).toBeTruthy();
    expect(await listedIds()).toEqual([]);

    // 重新打开设置页，在新地址按获取：新表生效，旧厂商的模型不再出现。
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    await expect(discover.locator('[data-settings="discover-count"]')).toContainText('1 个是新的');
    await expect(discover.locator('label', { hasText: 'vendor-new' })).toBeVisible();
    await expect(discover.locator('label', { hasText: 'vendor-old' })).toHaveCount(0);
    expect(await listedIds()).toEqual(['deepseek-flash', 'vendor-new']);
  } finally {
    await first.close();
    await second.close();
    await fixture.close();
  }
});

/// 页面编辑器里换 API 地址、重填密钥再保存：这条走 connection-save（不是 connection-update），
/// 旧地址存下的厂商列表也要当场作废；在新地址按「获取可用模型」才用新表。
test('编辑器换地址重填密钥后旧厂商列表作废，获取后新表生效', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  const first = await startVendor(['deepseek-flash', 'vendor-old']);
  const second = await startVendor(['deepseek-flash', 'vendor-new']);
  const listedIds = async () => {
    const response = await apiPost(request, fixture, { method: 'model-settings' });
    const listed = (await response.json()).listed as Record<string, { models: { id: string }[] }>;
    return listed.deepseek?.models.map((row) => row.id) ?? [];
  };
  try {
    const toFirst = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: first.url,
      model: 'deepseek-flash',
    });
    expect(toFirst.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    const edit = page.getByRole('button', { name: '编辑', exact: true }).first();
    await edit.click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    // 内置预设时「自定义设置」默认收起；换过地址后连接算自定义，默认展开。
    const openCustom = async () => {
      const toggle = editor.getByRole('button', { name: '自定义设置' });
      if ((await toggle.getAttribute('aria-expanded')) === 'false') await toggle.click();
    };
    await openCustom();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    await expect(editor.locator('[data-settings="discover"]')).toBeVisible({ timeout: 20_000 });
    expect(await listedIds()).toEqual(['deepseek-flash', 'vendor-old']);

    // 换地址、重填密钥，按页面「保存」：有密钥，页面走 connection-save。
    await editor.locator('[data-settings="endpoint"]').fill(second.url);
    await editor.locator('#key-deepseek').fill('sk-e2e-rewritten-key');
    await editor.getByRole('button', { name: '保存', exact: true }).click();
    // 保存成功后编辑器关掉；旧地址存下的列表不能还在。
    await expect(editor).toHaveCount(0, { timeout: 20_000 });
    await expect.poll(listedIds, { timeout: 20_000 }).toEqual([]);

    // 在新地址按获取：新表生效，旧厂商的模型不再出现。
    await edit.click();
    await expect(editor).toBeVisible();
    await openCustom();
    await editor.getByRole('button', { name: '获取可用模型' }).click();
    const discover = editor.locator('[data-settings="discover"]');
    await expect(discover).toBeVisible({ timeout: 20_000 });
    await expect(discover.locator('label', { hasText: 'vendor-new' })).toBeVisible();
    await expect(discover.locator('label', { hasText: 'vendor-old' })).toHaveCount(0);
    expect(await listedIds()).toEqual(['deepseek-flash', 'vendor-new']);
  } finally {
    await first.close();
    await second.close();
    await fixture.close();
  }
});

/// 新连接还没有 Keyring 记录：不能假装去问厂商；按钮不可用，并写明先保存连接。
test('新连接不能获取可用模型：按钮不可用并写明原因', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '添加模型提供商' }).click();
    const add = page.locator('[data-settings="add"]');
    await expect(add).toBeVisible();
    await add.getByText('自定义设置').click();
    const button = add.getByRole('button', { name: '获取可用模型' });
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute('title', '保存连接后，才能向厂商要真实列表');
    await expect(add).toContainText('保存连接后，才能向厂商要真实列表；现在列的是内置目录。');
  } finally {
    await fixture.close();
  }
});
