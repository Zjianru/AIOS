import { test, expect, type Page, type Route } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const EVIDENCE = process.env.AIOS_DOD_EVIDENCE;
void EVIDENCE;

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

test('第一次填 key：接上后是空对话', async ({ page }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await rewriteConnectionEndpoint(page, fixture);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText('先接一个模型，我们就能开始了')).toBeVisible();
    await page.locator('#first-use-key').fill('good-key');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    await expect(page.getByText('连接可用')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

/// T016.1：第一次接 MiniMax（国际站）：它成为当前模型，档取模型自己的默认档（medium），
/// 页面状态与选择器都跟着新连接。
test('第一次填 key：MiniMax 连接成为当前模型并用模型默认档', async ({ page, request }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await rewriteConnectionEndpoint(page, fixture);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: 'MiniMax（国际站）', exact: true }).click();
    await page.locator('#first-use-key').fill('good-key');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    await expect(page.getByText('连接可用')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 20_000 });
    // 选择器显示新连接上的模型和它自己的默认档（中 = medium，不是写死的 high）。
    const picker = page.locator('[data-kind="model-picker"]');
    await expect(picker).toContainText('MiniMax-M3.1-Flash', { timeout: 20_000 });
    await expect(picker).toContainText('中', { timeout: 20_000 });
    // 服务端 current/ready 与页面选择一致。
    const status = await apiPost(request, fixture, { method: 'status' });
    expect(status.ok()).toBeTruthy();
    const body = await status.json();
    expect(body.stats.connection).toBe('minimax');
    expect(body.stats.model).toBe('MiniMax-M3.1-Flash');
    expect(body.stats.effortWanted).toBe('medium');
  } finally {
    await fixture.close();
  }
});

test('第一次填 key：错 key 显示原因并可再试', async ({ page }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await rewriteConnectionEndpoint(page, fixture);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 20_000 });
    await page.locator('#first-use-key').fill('bad-key');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    await expect(page.getByText('连接不可用', { exact: false })).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole('button', { name: '再试一次', exact: true })).toBeVisible();
    await page.locator('#first-use-key').fill('good-key');
    await page.getByRole('button', { name: '再试一次', exact: true }).click();
    await expect(page.getByText('连接可用')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});
