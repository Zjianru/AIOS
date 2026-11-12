import { existsSync, renameSync } from 'node:fs';
import path from 'node:path';
import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

async function openSettings(page: Page, fixture: KernelFixture): Promise<void> {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
}

function breakIdentity(fixture: KernelFixture): { moved: string } {
  const original = path.join(fixture.dataDir, 'identity.txt');
  const moved = path.join(fixture.dataDir, 'moved-identity.txt');
  renameSync(original, moved);
  return { moved };
}

test('Keyring 读不出：横幅 + 指到新位置恢复', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    const { moved } = breakIdentity(fixture);
    await openSettings(page, fixture);
    const banner = page.locator('[data-settings="keyring-broken"]');
    await expect(banner).toBeVisible({ timeout: 20_000 });
    await expect(banner.getByText('Keyring 现在读不出来')).toBeVisible();
    await banner.getByRole('button', { name: '指到它现在的位置' }).click();
    await banner.getByPlaceholder('钥匙文件现在的完整路径，比如 ~/Backups/keyring.key').fill(moved);
    await banner.getByRole('button', { name: '用这个' }).click();
    await expect(page.locator('[data-settings="keyring-broken"]')).toHaveCount(0, { timeout: 20_000 });
    await expect(page.getByText('DeepSeek', { exact: true }).first()).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('Keyring 读不出：确认后重新开始', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    breakIdentity(fixture);
    await openSettings(page, fixture);
    await expect(page.locator('[data-settings="keyring-broken"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '找不回了，重新开始' }).click();
    await page.getByRole('button', { name: '重新开始', exact: true }).click();
    await expect(page.locator('[data-settings="keyring-broken"]')).toHaveCount(0, { timeout: 20_000 });
    await expect(page.getByText('还没有接任何模型。接上一个，Kind 才能开始做事。')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('设置页换钥匙位置 + 签发记录', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await openSettings(page, fixture);
    // Keyring 里的时间都显示成本地可读时间，不出现 epoch 秒。
    const keyringSection = page.locator('[data-settings="keyring"]');
    await expect(page.locator('[data-settings="model"]')).toContainText(
      /已保存在 Keyring（\d{4}-\d{2}-\d{2} \d{2}:\d{2}）/,
    );
    await expect(keyringSection).toContainText(
      /现在是一个文件，\d{4}-\d{2}-\d{2} \d{2}:\d{2} 起放在这里/,
    );
    // 手动「验证」走弹窗：默认用连接当前的模型，成功是弹窗里的对勾动画。
    await page.getByRole('button', { name: '验证', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('[data-verify="model-select"]')).toHaveValue('deepseek-flash');
    await dialog.getByRole('button', { name: '验证', exact: true }).click();
    await expect(dialog.locator('[data-verify="ok"]')).toBeVisible({ timeout: 20_000 });
    await expect(dialog.locator('[data-verify="check"]')).toHaveCSS('animation-name', 'verify-check-pop');
    // 弹窗路径不写连接 check：关掉后这一行还是「还没验证过」。
    await dialog.getByRole('button', { name: '关闭', exact: true }).click();
    await expect(page.getByText('还没验证过')).toBeVisible();
    await expect(page.getByText('今天签发', { exact: false })).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '看最近的' }).click();
    await expect(page.getByText('connection-verify')).toBeVisible();
    const issuedAt = keyringSection.locator('tbody tr').first().locator('td').first();
    await expect(issuedAt).toHaveText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
    await expect(keyringSection).not.toContainText(/\b1[6-9]\d{8}\b/);

    const relocated = path.join(fixture.dataDir, 'relocated-identity.txt');
    await page.getByRole('button', { name: '换位置', exact: true }).click();
    const input = page.locator('[data-settings="keyring"] input').first();
    await input.fill(relocated);
    await page.getByRole('button', { name: '挪过去' }).click();
    await expect.poll(() => existsSync(relocated), { timeout: 20_000 }).toBe(true);
    await expect.poll(() => existsSync(path.join(fixture.dataDir, 'identity.txt'))).toBe(false);
  } finally {
    await fixture.close();
  }
});
