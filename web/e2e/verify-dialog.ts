/**
 * 连接验证弹窗：点「验证」打开、等模型列表、选一个模型、验证、关掉。
 *
 * T034.2 v3：弹窗打开时**不预选**任何模型，用户选一个才验证；列表是弹窗自己去
 * available-models 拉的（不是设置里存的那份）。
 */
import { expect, type Locator, type Page } from '@playwright/test';

/** 设置页里的一条连接（行上有「验证」「编辑」）。 */
export function connectionRow(page: Page, connectionId: string): Locator {
  return page.locator(`[data-settings="connection"][data-connection="${connectionId}"]`);
}

/** 等弹窗把模型列表拿到：列表没到之前下拉是禁用的。 */
export async function waitVerifyModels(dialog: Locator): Promise<void> {
  await expect(dialog.locator('[data-verify="model-select"]')).toBeEnabled({ timeout: 20_000 });
}

/** 点行上的「验证」，返回已经打开、模型列表也拿到的弹窗。 */
export async function openVerifyDialog(page: Page, connectionId?: string): Promise<Locator> {
  const row = connectionId
    ? connectionRow(page, connectionId)
    : page.locator('[data-settings="connection"]').first();
  await row.getByRole('button', { name: '验证', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await waitVerifyModels(dialog);
  return dialog;
}

/** 选一个模型（弹窗没有预选，验证前必须先选）。 */
export async function selectVerifyModel(dialog: Locator, model: string): Promise<void> {
  await dialog.locator('[data-verify="model-select"]').selectOption(model);
}

/** 选模型验证一次，确认成功对勾后关闭。 */
export async function verifyConnectionViaDialog(
  page: Page,
  connectionId?: string,
  model = 'deepseek-flash',
): Promise<void> {
  const dialog = await openVerifyDialog(page, connectionId);
  await selectVerifyModel(dialog, model);
  await dialog.getByRole('button', { name: '验证', exact: true }).click();
  await expect(dialog.locator('[data-verify="ok"]')).toBeVisible({ timeout: 20_000 });
  await dialog.getByRole('button', { name: '关闭', exact: true }).click();
  await expect(dialog).toHaveCount(0);
}
