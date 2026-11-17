import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** 工具记忆小卡的真实事件回放：实时（过程组折叠）可见、撤销后刷新仍在同一轮。 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

test('工具记忆小卡：真实事件回放，不展开过程组也可见，撤销后刷新仍在原来那一轮', async ({ page }) => {
  const fixture = await startKernelFixture({ t062MemoryCard: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('记住，回答先给结论');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    const turn = page.locator('[data-turn]').filter({ hasText: '记住，回答先给结论' }).last();
    const chip = turn.locator('[data-kind="memo-chip"][data-memo-kind="remember"]');
    // 实时：过程组折叠着，小卡也该看得见（真实路径，不展开）。
    await expect(chip).toBeVisible({ timeout: 20_000 });
    await expect(chip).toContainText('记下了');
    await expect(chip).toContainText('回答风格：先给结论');

    // 不塞进回答正文或用户气泡。
    await expect(turn.locator('[data-kind="assistant-answer"] [data-kind="memo-chip"]')).toHaveCount(0);
    await expect(turn.locator('[data-kind="user-bubble"] [data-kind="memo-chip"]')).toHaveCount(0);

    // 撤销：小卡状态变「已撤销」。
    await chip.getByRole('button', { name: '撤销', exact: true }).click();
    await expect(chip).toContainText('已撤销', { timeout: 10_000 });

    // 刷新：小卡还在原来那一轮，状态保留。
    await page.reload();
    const reloadedTurn = page.locator('[data-turn]').filter({ hasText: '记住，回答先给结论' }).last();
    const reloadedChip = reloadedTurn.locator('[data-kind="memo-chip"][data-memo-kind="remember"]');
    await expect(reloadedChip).toContainText('已撤销', { timeout: 20_000 });
    await expect(reloadedTurn).toContainText('记住，回答先给结论');
  } finally {
    await fixture.close();
  }
});
