import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('设置页读真实 Keyring：空库没有连接', async ({ page }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText('还没有接任何模型。接上一个，Kind 才能开始做事。')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('设置页验证连接：真服务写注册器', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText('还没验证过')).toBeVisible();
    await page.getByRole('button', { name: '验证', exact: true }).click();
    await expect(page.getByText('连接可用', { exact: false })).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});
