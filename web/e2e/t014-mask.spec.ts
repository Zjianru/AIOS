import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const RULE = 'deepseek/deepseek-flash';

async function send(page: Page, text: string): Promise<void> {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/// 一个选项的屏蔽卡：只有「确认」「暂不」，选项默认选中；确认后设置页的已屏蔽列表里有它。
test('一个选项的屏蔽卡：只有确认和暂不，确认后设置页的已屏蔽列表里有它', async ({ page }) => {
  const fixture = await startKernelFixture({ t014Mask: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '把 deepseek-flash 屏蔽掉');

    const card = page.locator('[data-kind="confirm-card"]').first();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'pending');
    const option = card.locator('[data-action="card-option"]');
    await expect(option).toHaveCount(1);
    // 只有一个选项时它默认选中，用户只需要确认或暂不。
    await expect(option).toHaveAttribute('aria-checked', 'true');
    await expect(card.getByRole('button')).toHaveCount(2);
    await expect(card.getByRole('button', { name: '暂不', exact: true })).toBeVisible();
    const confirm = card.getByRole('button', { name: '确认', exact: true });
    await expect(confirm).toBeEnabled();
    // 事实是服务端读出的之前/之后。
    await expect(option.locator('[data-kind="confirm-fact"]')).toHaveCount(1);
    const fact = option.locator('[data-kind="confirm-fact"]').first();
    await expect(fact).toContainText('屏蔽状态');
    await expect(fact).toContainText('可用');
    await expect(fact).toContainText('已屏蔽');

    await confirm.click();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(card.locator('[data-kind="confirm-receipt"]')).toContainText('已执行');

    // 设置 › 模型与连接 › 已屏蔽的模型：规则在里面。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const section = page.locator('[data-settings="masked"]');
    await expect(section).toBeVisible({ timeout: 20_000 });
    await expect(section).toContainText(RULE);
    await expect(section.locator('[data-settings="mask-row"]').filter({ hasText: RULE })).toBeVisible();
  } finally {
    await fixture.close();
  }
});
