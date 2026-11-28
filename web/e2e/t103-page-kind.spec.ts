//! T103.1 设置页真 Kind：建议句发进同一 Kind 对话；settings_update 卡确认后设置变化。
import { expect, test } from '@playwright/test';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(240_000);

async function configureModels(request: any, fixture: any) {
  const settingsResponse = await apiPost(request, fixture, { method: 'model-settings' });
  const settings = (await settingsResponse.json()) as {
    settings: { connections: { id: string; endpoint: string; preset?: string }[] };
  };
  const connection = settings.settings.connections.find((item) => item.id === 'minimax-cn')!;
  await apiPost(request, fixture, {
    method: 'connection-update',
    id: connection.id,
    endpoint: connection.endpoint,
    preset: connection.preset,
    model: 'MiniMax-M3.1-Flash',
    models: [
      { id: 'MiniMax-M3.1-Flash', input: ['text'] },
      { id: 'MiniMax-M3', input: ['text', 'image'] },
    ],
  });
  await apiPost(request, fixture, { method: 'recheck' });
}

test('T103.1 设置页 Kind 加看图模型：卡确认后设置变、Kind 对话有这一轮', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await configureModels(request, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });

    await page.locator('[data-kind="page-kind-open"]').click();
    await expect(page.locator('[data-kind="page-kind"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="page-kind"]')).toContainText('在看：设置 · 模型与连接');

    const suggestion = page.locator('[data-kind="page-kind-suggestion"]', { hasText: '加一个看图模型' });
    await suggestion.click();
    await expect(page.locator('[data-kind="page-kind-input"]')).toHaveValue('');
    await expect(page.locator('[data-kind="page-kind-conversation"]')).toContainText('加一个看图模型', { timeout: 20_000 });
    const bubble = page.locator('[data-kind="page-kind-conversation"] [data-kind="user-bubble"]').last();
    await expect(bubble).toHaveText('加一个看图模型');
    await expect(bubble).not.toContainText('page-context');
    await expect(bubble).not.toContainText('在看');

    const card = page.locator('[data-kind="confirm-card"]').last();
    try {
      await expect(card).toBeVisible({ timeout: 90_000 });
    } catch (error) {
      const body = await page.locator('[data-kind="page-kind-conversation"]').innerText();
      throw new Error('设置页 Kind 没有出确认卡；对话：' + body.slice(0, 3000));
    }
    await card.locator('[data-action="card-confirm"]').click();
    await expect(section.locator('[data-kind="vision-model"]')).toHaveCount(1, { timeout: 30_000 });

    await page.locator('[data-kind="page-kind-suggestion"]', { hasText: '打开截桌面' }).click();
    const screenCard = page.locator('[data-kind="confirm-card"]').last();
    await expect(screenCard).toBeVisible({ timeout: 90_000 });
    await screenCard.locator('[data-action="card-confirm"]').click();
    await expect(section.locator('[data-kind="screen-toggle"]')).toHaveAttribute('data-state', 'checked', { timeout: 30_000 });

    // 回到 Kind：这句用户消息、卡和回执都在同一条根 Kind 对话里。
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
    const kindBubble = page.locator('[data-kind="user-bubble"]').filter({ hasText: '加一个看图模型' }).last();
    await expect(kindBubble).toHaveText('加一个看图模型', { timeout: 30_000 });
    await expect(page.locator('body')).not.toContainText('page-context');
  } finally {
    await fixture.close();
  }
});
