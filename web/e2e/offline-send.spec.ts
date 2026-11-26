import { test, expect } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

// T090.1：断线时发送不能静默丢；字留在输入框并提示连上后再发。

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture({ proxy: true });
});

test.afterAll(async () => {
  await fixture?.close();
});

test('断线发送：字留在输入框，提示「没连上内核，字还在，连上后再发」', async ({ page }) => {
  await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
  const input = page.locator('[data-kind="composer-input"]');
  await expect(input).toBeVisible({ timeout: 15_000 });

  // 先正常发一句，让页面和服务之间的代理连接真的建起来，再断开。
  await input.fill('断线前的一句');
  await page.keyboard.press('Enter');
  await expect(page.getByText('断线前的一句')).toBeVisible({ timeout: 15_000 });

  await fixture.dropProxy!();
  await expect(page.locator('[data-kind="offline"]')).toBeVisible({ timeout: 15_000 });

  await input.fill('断线时打的字');
  await page.keyboard.press('Enter');
  await expect(page.getByText('没连上内核，字还在，连上后再发')).toBeVisible({ timeout: 10_000 });
  await expect(input).toHaveValue('断线时打的字');
});
