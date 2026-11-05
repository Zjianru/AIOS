import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const send = async (page: import('@playwright/test').Page, text: string) => {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
};

/// T013.6 清单 12：我的话在右边气泡里；Kind 的回答不加气泡；停在消息上出复制按钮。
test('气泡：我的话在右侧气泡、Kind 回答没有气泡、复制原文', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await send(page, '你好呀');
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 30_000 });
    // 我的话：右侧气泡。
    const bubble = page.locator('[data-kind="user-bubble"]').first();
    await expect(bubble).toBeVisible();
    await expect(bubble).toContainText('你好呀');
    // 回答没有气泡。
    await expect(page.locator('[data-kind="assistant-bubble"]')).toHaveCount(0);
    // 位置：气泡在回答的右侧（回答左对齐）。
    const answer = page.getByText('（T007 e2e 假回放）').first();
    const bubbleBox = await bubble.boundingBox();
    const answerBox = await answer.boundingBox();
    expect(bubbleBox && answerBox ? bubbleBox.x > answerBox.x : false).toBe(true);
    // 停在消息上出复制按钮；点一下复制原文。
    await bubble.hover();
    const copy = bubble.locator('[data-kind="copy"]').first();
    await expect(copy).toBeVisible();
    await copy.click();
    await expect(copy).toHaveAttribute('aria-label', '已复制');
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toBe('你好呀');
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 12：代码块右上角复制，复制出来是原文。
test('代码块右上角复制：复制原文', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const fixture = await startKernelFixture({ codeBlock: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '给我一段代码');
    await expect(page.getByText('const answer = 42;')).toBeVisible({ timeout: 30_000 });
    const copy = page.locator('[data-kind="code-copy"]').first();
    await expect(copy).toBeVisible();
    await copy.click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toContain('const answer = 42;');
    expect(copied.startsWith('const answer')).toBe(true);
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 16：设置入口在侧栏左下角。
test('设置入口在侧栏左下角', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const footer = page.locator('[data-kind="sidebar-footer"]');
    await expect(footer).toBeVisible();
    await expect(footer.getByRole('button', { name: '设置', exact: true })).toBeVisible();
    // 侧栏内容区（上半部分）没有设置入口。
    await expect(page.locator('[data-kind="sidebar-content"]').getByRole('button', { name: '设置', exact: true })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T013.6 清单 16 + C10 守护：空会话没有停止键、发送按钮是灰的。
test('C10：空会话没有停止键、发送灰', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0);
    const sendButton = page.locator('[data-kind="send"]');
    await expect(sendButton).toHaveAttribute('aria-disabled', 'true');
  } finally {
    await fixture.close();
  }
});
