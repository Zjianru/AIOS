import { test, expect } from '@playwright/test';
import { startT013DodFixture } from './service-fixture';

// 空库 → 首次使用页加真 MiniMax 连接 → 「开始」→ recheck 起 root（修法 A）→ Kind 有输入框、能发消息。
// key 从 env 读（T008/T010/T012 的 DoD 做法；CI 无 key 时 skip）。
test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY');
test.setTimeout(300_000);

test('空库加连接后 root 恢复：Kind 有 composer 能发消息', async ({ page }) => {
  const fixture = await startT013DodFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 60_000 });
    await page.getByRole('button', { name: 'MiniMax（国内站）' }).click();
    await page.locator('#first-use-key').fill(process.env.MINIMAX_API_KEY ?? '');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    // 保存（修法 A：root 还 unavailable，所以新连接会被设为当前选择）。
    await expect(page.getByText('连接可用', { exact: false })).toBeVisible({ timeout: 180_000 });
    // 修法 C：onConnected → recheck → root Ok。
    await page.getByRole('button', { name: '开始', exact: true }).click();
    const retry = page.getByRole('button', { name: '现在重试' });
    if ((await retry.count()) > 0) await retry.first().click();
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 120_000 });
    await page.locator('[data-kind="composer-input"]').fill('只回一句：DOD13-ROOT-OK');
    await page.locator('[data-kind="send"]').click();
    await expect(page.getByText('DOD13-ROOT-OK', { exact: false }).last()).toBeVisible({ timeout: 180_000 });
  } finally {
    await fixture.close();
  }
});
