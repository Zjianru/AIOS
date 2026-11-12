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

test('设置页保存连接时拉模型列表：真服务写 check', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    // 预置连接已保存、还没拉过列表：行上没有「验证」按钮。
    await expect(page.getByText(/已保存在 Keyring（\d{4}-\d{2}-\d{2} \d{2}:\d{2}）/)).toBeVisible();
    await expect(page.getByRole('button', { name: '验证', exact: true })).toHaveCount(0);
    // 保存一把新 key（connection-save）时顺手拉一次厂商列表，结果写进 check。
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.locator('#key-deepseek').fill('new-good-key');
    await page
      .locator('[data-settings="editor"]')
      .getByRole('button', { name: '保存', exact: true })
      .click();
    await expect(page.getByText(/已拿到模型列表（\d{4}-\d{2}-\d{2} \d{2}:\d{2}）/)).toBeVisible({
      timeout: 20_000,
    });
  } finally {
    await fixture.close();
  }
});
