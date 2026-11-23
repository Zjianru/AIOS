import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('Kind 分条和表情：两块回答 + 气泡表情，两个工具不画过程步骤', async ({ page }) => {
  test.setTimeout(90_000);
  const fixture = await startKernelFixture({ t078Say: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill('谢谢，说说你的看法');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const parts = page.locator('[data-kind="assistant-say-part"]');
    await expect(parts).toHaveCount(2, { timeout: 30_000 });
    await expect(parts.nth(0)).toContainText('第一段');
    await expect(parts.nth(1)).toContainText('第二段');
    await expect(page.locator('[data-kind="kind-reaction"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="kind-reaction"]')).toHaveAttribute(
      'title',
      'Kind 回了 ❤️',
    );
    await expect(page.locator('[data-kind="assistant-answer"]')).not.toContainText('kind_say');
    await expect(page.locator('[data-step-kind="tool"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
