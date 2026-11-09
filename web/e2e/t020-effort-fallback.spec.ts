import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function send(page: Page, text: string): Promise<void> {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/// 确认卡换到「没有当前档」的模型：选择器显示服务给的实际档，并标「按实际档」。
/// 卡路径没有 session-model-select 的 HTTP 回执，实际档只能从 card/decision 来。
/// 同一连接内换模型（MiniMax-M3.1-Flash 没有 none 档，服务按 low 发），不走跨连接重建。
test('确认卡换模型缺当前档：选择器显示实际档并提示按实际档', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t020Effort: true });
  try {
    // 当前模型和目标模型都在这个连接里：换模型不跨连接，确认卡只改下一轮。
    const minimax = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M2.7-highspeed',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key-minimax',
      preset: 'minimax-cn',
      models: [
        { id: 'MiniMax-M2.7-highspeed' },
        { id: 'MiniMax-M3.1-Flash' },
        // 回放的卡里另两项是 DeepSeek 模型；同一连接的可选模型要能解析它们，否则整张卡被拒。
        { id: 'deepseek-v4-pro' },
      ],
    });
    expect(minimax.ok()).toBeTruthy();
    // 当前要的档是 none；M3.1-Flash 只有 low/medium/high/xhigh/max，确认后服务按 low 发。
    const seeded = await apiPost(request, fixture, {
      method: 'session-model-select',
      connection: 'minimax-cn',
      model: 'MiniMax-M2.7-highspeed',
      effortWanted: 'none',
    });
    expect(seeded.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="model-picker"]')).toBeVisible({ timeout: 30_000 });
    await send(page, '把这条对话换成 MiniMax M3.1 Flash 试试');

    const card = page.locator('[data-kind="confirm-card"]').first();
    await expect(card).toHaveAttribute('data-state', 'pending', { timeout: 30_000 });
    // 第三项只换模型、不带档；换到的 M3.1-Flash 没有 none。
    await card.locator('[data-action="card-option"]').nth(2).click();
    await card.locator('[data-action="card-confirm"]').click();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(card.locator('[data-kind="confirm-receipt"]')).toContainText('已执行');

    // 实际档 low（「弱」）显示在选择器上，不再是想要的 none（「不思考」）；并标「按实际档」。
    const picker = page.locator('[data-kind="model-picker"]');
    await expect(picker).toContainText('MiniMax-M3.1-Flash', { timeout: 20_000 });
    await expect(picker.locator('[data-kind="effort-fallback"]')).toBeVisible({
      timeout: 20_000,
    });
    await expect(picker.locator('[data-kind="effort-now"]')).toContainText('弱');
    await expect(picker.locator('[data-kind="effort-now"]')).not.toContainText('不思考');
    // 同一条提示：为什么会变成实际档。
    await expect(page.locator('[data-sonner-toast]').getByText(/按实际档/)).toBeVisible({
      timeout: 20_000,
    });
  } finally {
    await fixture.close();
  }
});
