import { test, expect, type Page, type Route } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';
import { verifyConnectionViaDialog } from './verify-dialog';

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
    await expect(page.getByText('连接可用', { exact: true })).toBeVisible({ timeout: 20_000 });
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
    // 预置连接已保存、还没验证：信封是 done（已保存）。
    await expect(status).toHaveAttribute('data-card-state', 'done');
    await expect(page.getByText('还没验证过')).toBeVisible();
    // 手动「验证」走弹窗，成功只体现在弹窗里：连接行仍是 done / 还没验证过。
    await verifyConnectionViaDialog(page);
    await expect(status).toHaveAttribute('data-card-state', 'done');
    await expect(page.getByText('还没验证过')).toBeVisible();
  } finally {
    await fixture.close();
  }
});
