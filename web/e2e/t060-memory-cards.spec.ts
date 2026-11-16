import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** 程序来源的记忆小卡（许可/整理完/没跑成）按 kind 渲染，和回复分开，刷新从日志重建。 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

test('程序记忆卡：许可/整理完/没跑成各一张，和回复分开，刷新还在', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ memoryCards: true });
  try {
    await page.goto(urlOf(fixture));
    const chips = page.locator('[data-kind="memo-chip"]');
    await expect(chips).toHaveCount(3, { timeout: 20_000 });

    const permission = page.locator('[data-memo-kind="permission"]');
    await expect(permission).toHaveCount(1);
    await expect(permission).toContainText('做梦许可');
    await expect(permission).toHaveAttribute('data-ref', 'dream-run-1');

    const done = page.locator('[data-memo-kind="dream_done"]');
    await expect(done).toHaveCount(1);
    await expect(done).toContainText('整理完了');
    await expect(done).toHaveAttribute('data-ref', 'dream-run-2');

    const failed = page.locator('[data-memo-kind="dream_failed"]');
    await expect(failed).toHaveCount(1);
    await expect(failed).toContainText('整理没跑成');
    await expect(failed).toHaveAttribute('data-ref', 'dream-run-3');

    // 小卡是程序加的一行，不塞进用户气泡或 Kind 的回答里。
    await expect(page.locator('[data-kind="user-bubble"] [data-kind="memo-chip"]')).toHaveCount(0);
    await expect(
      page.locator('[data-kind="assistant-answer"] [data-kind="memo-chip"]'),
    ).toHaveCount(0);
    await expect(page.getByText('好，我先记着；整理的事到点再说。')).toBeVisible();

    // 刷新：卡从会话日志重建，不靠页面自己的状态。
    await page.reload();
    await expect(page.locator('[data-kind="memo-chip"]')).toHaveCount(3, { timeout: 20_000 });
    await expect(page.locator('[data-memo-kind="dream_done"]')).toContainText('整理完了');
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});
