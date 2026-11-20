import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** 两种记忆卡：替换你定的条目、关于你草稿；按钮点一次生效，状态从记忆库读。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('记忆卡：替换/两条都留、关于你保存/不用', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t073Cards: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '周报以后只写两段');
    const replace = page.locator('[data-memo-kind="replace"]');
    await expect(replace).toBeVisible({ timeout: 30_000 });
    await expect(replace).toContainText('这条会替换你定的「周报格式」');
    await expect(replace).toContainText('周报按三段写');
    await expect(replace).toContainText('周报只写两段');
    await replace.getByRole('button', { name: '替换' }).click();
    await expect(replace).toContainText('替换了「周报格式」', { timeout: 10_000 });

    await sendKind(page, '简单的事短点说，测试只用 MiniMax 跑');
    const draft = page.locator('[data-memo-kind="about_draft"]');
    await expect(draft).toBeVisible({ timeout: 30_000 });
    await expect(draft).toContainText('Kind 起草了「关于你」的 2 处改动');
    await expect(draft).toContainText('用中文回答，简单的短、复杂的讲透');
    await expect(draft).toContainText('测试只用 MiniMax 跑');
    await draft.getByRole('button', { name: '保存' }).click();
    await expect(draft).toContainText('「关于你」改了 2 处', { timeout: 10_000 });

    await page.goto(urlOf(fixture, '#memory'));
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible();
    await expect(page.locator('[data-kind="memory-about"]')).toContainText('测试只用 MiniMax 跑');
    await expect(page.locator('[data-kind="memory-about"]')).toContainText('复杂的讲透');
    // 失效的旧条目变淡、标被替换；新的变成你定的。
    const invalid = page.locator('[data-kind="memory-item"][data-invalid="replaced"]');
    await expect(invalid).toHaveCount(1);
    await expect(invalid).toContainText('已失效 · 被替换');
    const fresh = page.locator('[data-kind="memory-item"]', { hasText: '周报只写两段' }).first();
    await expect(fresh).toContainText('你定的');
  } finally {
    await fixture.close();
  }
});

test('记忆卡：两条都留、关于你不用都不动库', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t073Cards: true });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '周报以后只写两段');
    const replace = page.locator('[data-memo-kind="replace"]');
    await expect(replace).toBeVisible({ timeout: 30_000 });
    await replace.getByRole('button', { name: '两条都留' }).click();
    await expect(replace).toContainText('两条都留 · 「周报格式」照旧');

    await sendKind(page, '简单的事短点说，测试只用 MiniMax 跑');
    const draft = page.locator('[data-memo-kind="about_draft"]');
    await expect(draft).toBeVisible({ timeout: 30_000 });
    await draft.getByRole('button', { name: '不用' }).click();
    await expect(draft).toContainText('没用这份草稿 · 「关于你」没动');

    await page.goto(urlOf(fixture, '#memory'));
    await expect(page.locator('[data-kind="memory-about"]')).not.toContainText('测试只用 MiniMax 跑');
    await expect(page.locator('[data-kind="memory-item"][data-invalid="replaced"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
