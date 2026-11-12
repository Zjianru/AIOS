/**
 * 连接验证弹窗（手动「验证」）：打开弹窗、用默认模型验证、等成功对勾、关掉。
 * 失败路径的断言自己开弹窗做（要选模型、看原因），不经过这里。
 */
import { expect, type Locator, type Page } from '@playwright/test';

/** 设置页里的一条连接（行上有「验证」「编辑」）。 */
export function connectionRow(page: Page, connectionId: string): Locator {
  return page.locator(`[data-settings="connection"][data-connection="${connectionId}"]`);
}

/** 点行上的「验证」，返回已经打开的弹窗。 */
export async function openVerifyDialog(page: Page, connectionId?: string): Promise<Locator> {
  const row = connectionId
    ? connectionRow(page, connectionId)
    : page.locator('[data-settings="connection"]').first();
  await row.getByRole('button', { name: '验证', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  return dialog;
}

/** 默认模型验证一次，确认成功对勾后关闭。 */
export async function verifyConnectionViaDialog(page: Page, connectionId?: string): Promise<void> {
  const dialog = await openVerifyDialog(page, connectionId);
  await dialog.getByRole('button', { name: '验证', exact: true }).click();
  await expect(dialog.locator('[data-verify="ok"]')).toBeVisible({ timeout: 20_000 });
  await dialog.getByRole('button', { name: '关闭', exact: true }).click();
  await expect(dialog).toHaveCount(0);
}
