import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

async function openSettings(page: Parameters<typeof test>[1] extends never ? never : import('@playwright/test').Page, fixture: import('./service-fixture').KernelFixture) {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
}

test('设置页撤销密钥：状态变没有密钥', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await openSettings(page, fixture);
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await expect(page.locator('[data-settings="editor"]')).toBeVisible();
    await page.getByRole('button', { name: '撤销密钥', exact: true }).click();
    await page.getByRole('button', { name: '撤销', exact: true }).click();
    await expect(page.getByText('还没有密钥。填上才能用这个提供商。').first()).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('设置页删除提供商：列表变空', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await openSettings(page, fixture);
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.getByRole('button', { name: '删除这个提供商', exact: true }).click();
    await page.getByRole('button', { name: '删除', exact: true }).click();
    await expect(page.getByText('还没有接任何模型。接上一个，Kind 才能开始做事。')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('设置页换 key：保存时拉模型列表', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.locator('#key-deepseek').fill('new-good-key');
    // 页面上还有一个「保存」在联网搜索一节（填 Exa 的 key）；这里限定连接编辑卡。
    await page
      .locator('[data-settings="editor"]')
      .getByRole('button', { name: '保存', exact: true })
      .click();
    await expect(page.locator('[data-settings="editor"]')).toHaveCount(0, { timeout: 20_000 });
    await expect(page.getByText(/已拿到模型列表（/)).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole('button', { name: '验证', exact: true })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
