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
/// 当前是 none，V4 Pro 只有 low/high/max；同一连接换模型，不走跨连接重建。
test('确认卡换模型缺当前档：选择器显示实际档并提示按实际档', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t014Confirm: true });
  try {
    // 卡里第三项指向 MiniMax 连接；先存一个只有这个模型的连接，服务端才认得这一项。
    const minimax = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M2.7-highspeed',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key-minimax',
      preset: 'minimax-cn',
      models: [{ id: 'MiniMax-M2.7-highspeed' }],
    });
    expect(minimax.ok()).toBeTruthy();
    // 连接改成「跟着厂商走」，可选模型才来自目录（V4 Pro 才能进卡）。
    const followVendor = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'deepseek',
      name: 'DeepSeek',
      model: 'deepseek-flash',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key',
      preset: 'deepseek',
      models: [],
    });
    expect(followVendor.ok()).toBeTruthy();
    // 当前要的档是 minimal；DeepSeek 的档位没有 minimal，服务按最近的 low 发。
    const seeded = await apiPost(request, fixture, {
      method: 'session-model-select',
      connection: 'deepseek',
      model: 'deepseek-flash',
      effortWanted: 'minimal',
    });
    expect(seeded.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-kind="model-picker"]')).toBeVisible({ timeout: 30_000 });
    await send(page, '把这条对话换成 V4 Pro 试试');

    const card = page.locator('[data-kind="confirm-card"]').first();
    await expect(card).toHaveAttribute('data-state', 'pending', { timeout: 30_000 });

    // 把连接改回 DeepSeek 的真实地址：出站不会发生，只让服务按 DeepSeek 的档位规则算实际档
    // （minimal → low）。卡已经出完，回放不受影响。
    const updated = await apiPost(request, fixture, {
      method: 'connection-update',
      id: 'deepseek',
      endpoint: 'https://api.deepseek.com/anthropic',
      model: 'deepseek-flash',
      preset: 'deepseek',
      models: [],
    });
    expect(updated.ok()).toBeTruthy();

    // 第一项只换模型、不带档；换到的 V4 Pro 也没有 minimal。
    await card.locator('[data-action="card-option"]').nth(0).click();
    await card.locator('[data-action="card-confirm"]').click();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(card.locator('[data-kind="confirm-receipt"]')).toContainText('已执行');

    // 实际档 low（「弱」）显示在选择器上，不再是想要的 minimal（「最弱」）；并标「按实际档」。
    const picker = page.locator('[data-kind="model-picker"]');
    await expect(picker).toContainText('V4 Pro', { timeout: 20_000 });
    await expect(picker.locator('[data-kind="effort-fallback"]')).toBeVisible({
      timeout: 20_000,
    });
    await expect(picker.locator('[data-kind="effort-now"]')).toContainText('弱');
    await expect(picker.locator('[data-kind="effort-now"]')).not.toContainText('最弱');
    // 同一条提示：为什么会变成实际档。
    await expect(page.locator('[data-sonner-toast]').getByText(/按实际档/)).toBeVisible({
      timeout: 20_000,
    });
  } finally {
    await fixture.close();
  }
});
