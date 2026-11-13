import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T052.1：一句话 → 确认卡（每项一行事实）→ 确认 → 第一项开跑，第二项等上游。 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

test('任务图：先查再写 → 确认卡两行事实 → 确认 → 第一项开跑', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t052Graph: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('先查 notes.md 有几行，再把行数写进 other.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    // 确认卡：两个子任务各一行事实（标题 + 等谁 + 写入范围）。
    const card = page.locator('[data-kind="confirm-card"]').first();
    await expect(card).toBeVisible({ timeout: 40_000 });
    const facts = card.locator('[data-kind="confirm-fact"]');
    await expect(facts).toHaveCount(2);
    await expect(facts.nth(0)).toContainText('先查行数');
    await expect(facts.nth(0)).toContainText('等：无');
    await expect(facts.nth(0)).toContainText('写入范围：只读');
    await expect(facts.nth(1)).toContainText('再写结果');
    await expect(facts.nth(1)).toContainText('等：先查行数');
    await expect(facts.nth(1)).toContainText('写入范围：other.txt');

    // 确认：卡片收成回执。
    await card.locator('[data-action="card-option"]').first().click();
    await card.locator('[data-action="card-confirm"]').click();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 40_000 });

    // 第一项开跑并报回：Kind 对话里出现它的结果卡（标题「先查行数」）。
    const firstCard = page
      .locator('[data-kind="task-result"]')
      .filter({ hasText: '先查行数' })
      .first();
    await expect(firstCard).toBeVisible({ timeout: 60_000 });
    // 第二项不会在没有上游结果时自己跑：它的张数还不能出现（只有第一项有卡）。
    const secondCards = page
      .locator('[data-kind="task-result"]')
      .filter({ hasText: '再写结果' });
    await expect(secondCards).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
