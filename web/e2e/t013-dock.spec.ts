import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** T012 F1：四类单价都填上，让花费 pill 出现（和 t012-page/t013-tooltip 的用例一致）。 */
async function setUserPrices(
  request: import('@playwright/test').APIRequestContext,
  fixture: { fakeEndpoint: string },
) {
  const response = await apiPost(request, fixture as never, {
    method: 'connection-update',
    id: 'deepseek',
    endpoint: fixture.fakeEndpoint,
    model: 'deepseek-flash',
    models: [
      {
        id: 'deepseek-flash',
        priceInput: 0.15,
        priceOutput: 0.6,
        priceCacheRead: 0.003,
        priceCacheWrite: 0.1,
      },
    ],
  });
  expect(response.ok()).toBeTruthy();
}

/// T013 验收 H2：输入栏状态行（轮/步、↑↓、缓存命中、花费、上下文）统一字体与间距：
/// 行内数字不再用 brand-type，数字与文字同族同字号同颜色；图标同尺寸、与文字同一垂直中心。
test('状态行（验收 H2）：数字/文字同字体，图标与文字同一垂直中心', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageReadOnly: true });
  try {
    await setUserPrices(request, fixture);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });

    const dock = page.locator('[data-kind="composer-dock"]');
    await expect(dock).toBeVisible();
    await expect(page.locator('[data-kind="stats-cost"]')).toContainText('$', { timeout: 15_000 });
    await expect(page.locator('[data-kind="stats-usage"]')).toContainText('缓存命中', { timeout: 15_000 });

    const audit = await dock.evaluate((root) => {
      const rectOf = (element: Element) => {
        const rect = element.getBoundingClientRect();
        return { top: rect.top, bottom: rect.bottom, height: rect.height, center: rect.top + rect.height / 2 };
      };
      const styleOf = (element: Element) => {
        const style = getComputedStyle(element);
        return {
          family: style.fontFamily,
          size: style.fontSize,
          weight: style.fontWeight,
          color: style.color,
          variant: style.fontVariantNumeric,
        };
      };
      const pills = [
        ...root.querySelectorAll(
          '[data-kind="stats-activity"], [data-kind="stats-usage"], [data-kind="stats-cost"], [data-kind="context-meter"]',
        ),
      ];
      const numbers = [...root.querySelectorAll('[data-kind="stat-number"]')];
      const texts = [...root.querySelectorAll('[data-kind="stat-text"]')];
      const separators = [...root.querySelectorAll('[data-kind="stat-sep"]')];
      const iconSizes = pills.flatMap((pill) =>
        [...pill.querySelectorAll('svg')].map((icon) => {
          const rect = icon.getBoundingClientRect();
          return { width: rect.width, height: rect.height };
        }),
      );
      const childCenters = pills.flatMap((pill) => {
        const pillCenter = rectOf(pill).center;
        return [...pill.children].map((child) => ({
          kind: child.getAttribute('data-kind') ?? child.tagName.toLowerCase(),
          delta: Math.abs(rectOf(child).center - pillCenter),
        }));
      });
      return {
        rootStyle: styleOf(root),
        brandCount: root.querySelectorAll('.brand-type').length,
        numbers: numbers.map((element) => ({ ...styleOf(element), ...rectOf(element) })),
        texts: texts.map((element) => ({ ...styleOf(element), ...rectOf(element) })),
        iconSizes,
        childCenters,
        separators: separators.map((element) => ({ ...styleOf(element), ...rectOf(element), display: getComputedStyle(element).display })),
      };
    });

    // 行内没有 brand-type（brand 只留给独立大数字）。
    expect(audit.brandCount).toBe(0);
    expect(audit.numbers.length).toBeGreaterThanOrEqual(6);
    expect(audit.texts.length).toBeGreaterThanOrEqual(3);
    // 数字 span 与文字 span 同一字体、同字号、同字重；全部继承正文 sans。
    for (const number of audit.numbers) {
      expect(number.family).toBe(audit.rootStyle.family);
      expect(number.size).toBe(audit.rootStyle.size);
      expect(number.weight).toBe(audit.rootStyle.weight);
      expect(number.variant).toBe('tabular-nums');
    }
    for (const text of audit.texts) {
      expect(text.family).toBe(audit.rootStyle.family);
      expect(text.size).toBe(audit.numbers[0].size);
      expect(text.weight).toBe(audit.numbers[0].weight);
      expect(text.color).toBe(audit.numbers[0].color);
    }
    // 图标统一 14px，且和所在 pill 同一垂直中心（≤2px）。
    expect(audit.iconSizes.length).toBeGreaterThanOrEqual(4);
    expect(
      audit.iconSizes.every((icon) => Math.abs(icon.width - 14) <= 0.5 && Math.abs(icon.height - 14) <= 0.5),
    ).toBe(true);
    expect(audit.childCenters.length).toBeGreaterThanOrEqual(6);
    expect(audit.childCenters.every((child) => child.delta <= 2)).toBe(true);
    // 分隔点用 flex 居中，不靠 JSX 空白凑间距。
    expect(audit.separators.length).toBeGreaterThanOrEqual(2);
    expect(audit.separators.every((separator) => separator.display === 'flex')).toBe(true);
  } finally {
    await fixture.close();
  }
});
