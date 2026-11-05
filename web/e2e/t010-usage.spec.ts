import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/// T010.3 清单 4/5/6 + 故事 1/3：悬停行给模型名、全部输入、缓存命中、输出；展开给想了 N 秒。
test('每轮用量：模型名、全部输入、缓存命中、输出、想了 N 秒', async ({ page }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    // 悬停「工作过程」才显示用量行；输入 = 1000 + 200 + 50 = 1250；命中 = 200/1250 = 16%。
    await page.getByText('工作过程', { exact: false }).last().hover();
    await expect(
      page.getByText('DeepSeek Flash · 输入 1250 · 缓存命中 16% · 输出 120', { exact: false }),
    ).toBeVisible();
    await page.getByText('工作过程', { exact: false }).last().click();
    await expect(page.getByText('想了 1 秒', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});

/// T010.3 故事 2：厂商没报缓存的那一轮，悬停行没有「缓存命中」；没思考就不写「想了 N 秒」。
test('没报缓存、没思考：不出现缓存命中与想了 N 秒', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await page.getByText('工作过程', { exact: false }).last().hover();
    await expect(page.locator('[data-kind="assistant-answer"]').getByText('缓存命中', { exact: false })).toHaveCount(0);
    await page.getByText('工作过程', { exact: false }).last().click();
    await expect(page.getByText('想了', { exact: false })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
