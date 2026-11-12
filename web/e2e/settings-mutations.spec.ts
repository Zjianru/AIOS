import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';
import { verifyConnectionViaDialog, waitVerifyModels } from './verify-dialog';

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

test('设置页换 key：保存后自动弹窗选模型，旧的自动验证照旧', async ({ page }) => {
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
    // 保存新 key 后：验证弹窗自动打开（还没有预选），不用再点行上的「验证」。
    await expect(page.locator('[data-settings="editor"]')).toHaveCount(0, { timeout: 20_000 });
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 20_000 });
    await waitVerifyModels(dialog);
    await expect(dialog.locator('[data-verify="model-select"]')).toHaveValue('');
    // 保存新 key 后的自动验证仍走旧路径：连接行变成可用。
    await expect(page.getByText('连接可用', { exact: false })).toBeVisible({ timeout: 20_000 });
    // 关掉弹窗，再从行上的「验证」打开同一个弹窗；这次成功不改上面那条连接状态。
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await verifyConnectionViaDialog(page);
    await expect(page.getByText('连接可用', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});
