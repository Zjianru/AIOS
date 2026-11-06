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
    const usageRow = process.locator('[data-kind="turn-usage"]');
    await expect(usageRow).toContainText('DeepSeek-V4.1-Flash');
    await expect(usageRow).toContainText('输入');
    await expect(usageRow.locator('[data-kind="usage-number"]')).toHaveText(['1250', '16%', '120']);
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

/// T013 验收 H1：悬停用量行要跟左边「已完成，用时」同一垂直中心（≤3px）；
/// 整行（含中文）统一成正文 sans + tabular-nums——mono 里的中文会回退到另一种字体，数字和中文像两种字体。
test('用量行（验收 H1）：与工作过程行垂直对齐，整行 sans + tabular-nums', async ({ page }) => {
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
    const [rowBox, buttonBox] = await Promise.all([
      row.boundingBox(),
      process.locator('button').first().boundingBox(),
    ]);
    if (!rowBox || !buttonBox) throw new Error('用量行或工作过程行没有可见尺寸');
    const centerOf = (box: { y: number; height: number }) => box.y + box.height / 2;
    expect(Math.abs(centerOf(rowBox) - centerOf(buttonBox))).toBeLessThanOrEqual(3);
    const metrics = await row.evaluate((element) => {
      const style = getComputedStyle(element);
      const button = element.closest('[data-kind="process"]')?.querySelector('button') ?? null;
      const numbers = [...element.querySelectorAll('[data-kind="usage-number"]')];
      return {
        pointerEvents: style.pointerEvents,
        family: style.fontFamily,
        size: style.fontSize,
        weight: style.fontWeight,
        variant: style.fontVariantNumeric,
        buttonFamily: button ? getComputedStyle(button).fontFamily : null,
        numbers: numbers.map((item) => {
          const numberStyle = getComputedStyle(item);
          return { family: numberStyle.fontFamily, size: numberStyle.fontSize, weight: numberStyle.fontWeight };
        }),
      };
    });
    expect(metrics.pointerEvents).toBe('none');
    expect(metrics.family).toBe(metrics.buttonFamily);
    expect(metrics.family).toContain('sans-serif');
    expect(metrics.variant).toBe('tabular-nums');
    expect(metrics.size).toBe('12px');
    expect(metrics.numbers.length).toBe(3);
    expect(
      metrics.numbers.every((n) => n.family === metrics.family && n.size === metrics.size && n.weight === metrics.weight),
    ).toBe(true);
  } finally {
    await fixture.close();
  }
});
