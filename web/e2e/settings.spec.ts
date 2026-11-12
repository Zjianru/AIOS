import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';
import { verifyConnectionViaDialog } from './verify-dialog';

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

test('设置页验证连接：弹窗选模型，结果只在弹窗里', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText('还没验证过')).toBeVisible();
    await verifyConnectionViaDialog(page);
    // 弹窗路径不写连接 check。
    await expect(page.getByText('还没验证过')).toBeVisible();
  } finally {
    await fixture.close();
  }
});
