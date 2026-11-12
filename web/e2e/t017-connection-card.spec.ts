import { test, expect, type Page, type Route } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T017.4：连接卡从信封渲染。
 * 首次使用页走 <Card>；设置页的连接状态行从同一份 connectionCardOf 的信封取。
 * 卡片的状态属性（data-card-state）是程序按 Keyring 状态定的，不由页面组件自己猜。
 */

/** 页面会向默认 DeepSeek endpoint 发连接；e2e 把它改写到假 DeepSeek（真产品不变）。 */
async function rewriteConnectionEndpoint(page: Page, fixture: KernelFixture): Promise<void> {
  await page.route(/\/api(\?|$)/, async (route: Route) => {
    const request = route.request();
    if (request.method() === 'POST') {
      const body = JSON.parse(request.postData() ?? '{}') as Record<string, unknown>;
      if (body.method === 'connection-save') {
        body.endpoint = fixture.fakeEndpoint;
        await route.continue({ postData: JSON.stringify(body) });
        return;
      }
    }
    await route.continue();
  });
}

test('首次使用：连接卡从未保存到可用都在信封里', async ({ page }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await rewriteConnectionEndpoint(page, fixture);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    const card = page.locator('[data-first-use="connect"][data-card="connection"]');
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card).toHaveAttribute('data-card-state', 'pending');
    await page.locator('#first-use-key').fill('good-key');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    await expect(page.getByText('已拿到 2 个模型', { exact: true })).toBeVisible({ timeout: 20_000 });
    await expect(card).toHaveAttribute('data-card-state', 'done');
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('设置页：连接状态行从连接卡信封取', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    const status = page.locator('[data-settings="model"] [data-card-state]').first();
    // 预置连接已保存、还没拉过列表：信封是 done（已保存），行上不再有「验证」按钮。
    await expect(status).toHaveAttribute('data-card-state', 'done');
    await expect(page.getByText(/已保存在 Keyring（\d{4}-\d{2}-\d{2} \d{2}:\d{2}）/)).toBeVisible();
    await expect(page.getByRole('button', { name: '验证', exact: true })).toHaveCount(0);
    // 保存一把新 key（走 connection-save）时顺手拉到列表：状态行写「已拿到模型列表」。
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.locator('#key-deepseek').fill('new-good-key');
    await page
      .locator('[data-settings="editor"]')
      .getByRole('button', { name: '保存', exact: true })
      .click();
    await expect(page.getByText(/已拿到模型列表（\d{4}-\d{2}-\d{2} \d{2}:\d{2}）/)).toBeVisible({
      timeout: 20_000,
    });
    await expect(status).toHaveAttribute('data-card-state', 'done');
  } finally {
    await fixture.close();
  }
});
