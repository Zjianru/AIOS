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
    // 等这一轮真的结束（用量/token 在 turn/end 后进视图，运行中还是「Kind 工作中」）。
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    // T013.5：悬停整轮工作过程行才显示用量；输入 = 1000 + 200 + 50 = 1250；命中 = 200/1250 = 16%。
    const process = page.locator('[data-kind="process"]').last();
    await process.hover();
    await expect(
      page.getByText('DeepSeek-V4.1-Flash · 输入 1250 · 缓存命中 16% · 输出 120', { exact: false }),
    ).toBeVisible();
    // 展开整轮行：「经过」行给出想了 N 秒。
    await process.locator('button').first().click();
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
    // 等这一轮真的结束（用量/token 在 turn/end 后进视图，运行中还是「Kind 工作中」）。
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    const process = page.locator('[data-kind="process"]').last();
    await process.hover();
    await expect(page.getByText('缓存命中', { exact: false })).toHaveCount(0);
    await process.locator('button').first().click();
    await expect(page.getByText('想了', { exact: false })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T013 验收 F3：悬停用量行的数字跟正文同字体（原来这几个数字用了 brand-type 的标识字体）。
test('用量行数字与正文字体一致', async ({ page }) => {
  const fixture = await startKernelFixture({ usageTurn: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });
    const process = page.locator('[data-kind="process"]').last();
    await process.hover();
    const row = process.locator('[data-kind="turn-usage"]');
    await expect(row).toBeVisible();
    const numbers = row.locator('[data-kind="usage-number"]');
    expect(await numbers.count()).toBeGreaterThan(0);
    const fonts = await numbers.evaluateAll((elements) =>
      elements.map((element) => getComputedStyle(element).fontFamily),
    );
    const bodyFont = await page.evaluate(() => getComputedStyle(document.body).fontFamily);
    expect(fonts.every((font) => font === bodyFont)).toBe(true);
  } finally {
    await fixture.close();
  }
});
