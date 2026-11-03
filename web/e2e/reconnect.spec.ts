import { test, expect } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture({ slowMs: 2000, proxy: true });
});

test.afterAll(async () => {
  await fixture?.close();
});

/// T007.9 清单 7：断开 WebSocket 再连：横幅出现又消失；断线期间的事件补齐、不重复。
test('断线重连：横幅出现又消失，事件补齐不重复', async ({ page }) => {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await page.getByPlaceholder('和 Kind 说…').fill('断线测试');
  await page.getByRole('button', { name: '发送' }).click();
  await expect(page.getByText('断线测试')).toBeVisible({ timeout: 15_000 });

  // 断开页面与服务之间的代理：WS 断，重连先失败；服务端这一轮照常跑完。
  await fixture.dropProxy?.();
  const banner = page.locator('[data-kind="offline"]');
  await expect(banner).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(3000);
  await fixture.restoreProxy?.();
  await expect(banner).toHaveCount(0, { timeout: 25_000 });
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 25_000 });
  await expect(page.getByText('断线测试')).toHaveCount(1);
});
