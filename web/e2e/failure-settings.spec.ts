import { renameSync } from 'node:fs';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('失败块：凭证原因给「去设置」', async ({ page }) => {
  const fixture = await startKernelFixture({ failure: 'credential' });
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="failure"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="failure"]')).toContainText('拒绝了这把密钥');
    await expect(page.locator('[data-kind="failure"]')).not.toContainText('AUTH');
    await page.getByRole('button', { name: /去设置/ }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('Keyring 读不出：失败块按 Keyring 原因并去设置', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    renameSync(path.join(fixture.dataDir, 'identity.txt'), path.join(fixture.dataDir, 'moved-identity.txt'));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="failure"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText('AIOS 不会改用别处的密钥', { exact: false })).toBeVisible();
    await page.getByRole('button', { name: /去设置/ }).click();
    await expect(page.locator('[data-settings="keyring-broken"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});
