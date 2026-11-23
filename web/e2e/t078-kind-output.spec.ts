import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

test('Kind 分条和表情：流式 1 块变 2 块 + 气泡表情，两个工具不画过程步骤', async ({ page }) => {
  test.setTimeout(90_000);
  const fixture = await startKernelFixture({ t078Say: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill('谢谢，说说你的看法');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const parts = page.locator('[data-kind="assistant-say-part"]');
    // 流式：第一条 parts 先成形，页面先画 1 块；delta 到齐后才 2 块。
    await expect.poll(async () => parts.count(), { timeout: 30_000 }).toBe(1);
    await expect(parts.nth(0)).toContainText('第一段');
    await expect(parts).toHaveCount(2, { timeout: 30_000 });
    // 第二块写到一半时已经出现（不是等整条写完才蹦出来）。
    await expect(parts.nth(1)).toContainText('第', { timeout: 30_000 });
    await expect(parts.nth(1)).toContainText('第二段', { timeout: 30_000 });
    await expect(page.locator('[data-kind="kind-reaction"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="kind-reaction"]')).toHaveAttribute(
      'title',
      'Kind 回了 ❤️',
    );
    await expect(page.locator('[data-step-kind="tool"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

test('Kind 只回表情、空文本：有表情、无回答块、无「正在想」、无工具步骤', async ({ page }) => {
  test.setTimeout(90_000);
  const fixture = await startKernelFixture({ t078ReactOnly: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.getByPlaceholder('和 Kind 说…').fill('只回个表情');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="kind-reaction"]')).toHaveCount(1, { timeout: 20_000 });
    await expect(page.locator('[data-kind="kind-reaction"]')).toHaveAttribute(
      'title',
      'Kind 回了 ❤️',
    );
    await expect(page.locator('[data-kind="assistant-answer"]')).toHaveCount(0);
    await expect(page.getByText('正在想…')).toHaveCount(0);
    await expect(page.locator('[data-step-kind="tool"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
