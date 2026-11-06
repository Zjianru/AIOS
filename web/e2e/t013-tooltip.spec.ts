import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** T012 F1：四类单价都填上（与 t012-page 的用例一致，保证花费有价格可算）。 */
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

/// T013 验收 F7：悬停花费出的卡片原来带一个白色菱形箭头（Arrow 用 bg-foreground），
/// 与深色卡片不搭。改成和卡片同背景/描边，或不再画独立箭头。
test('tooltip（验收 F7）：花费悬停卡不再有独立白箭头，背景与卡片一致', async ({ page, request }) => {
  const fixture = await startKernelFixture({ usageReadOnly: true });
  try {
    await setUserPrices(request, fixture);
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 60_000 });

    const cost = page.locator('[data-kind="stats-cost"]');
    await expect(cost).toContainText('$');
    await cost.hover();
    const tooltip = page.locator('[data-slot="tooltip-content"]').filter({ hasText: '这个会话的花费' });
    await expect(tooltip).toBeVisible({ timeout: 10_000 });

    // 要么不画箭头；画了就必须和卡片同色（背景和填充都不能是独立的白块）。
    const arrowCount = await tooltip.locator('svg').count();
    if (arrowCount > 0) {
      const colors = await page.evaluate(() => {
        const contents = [...document.querySelectorAll('[data-slot="tooltip-content"]')];
        const open = contents.find((element) => (element.textContent ?? '').includes('这个会话的花费'));
        if (!open) return null;
        const arrow = open.querySelector('svg');
        if (!arrow) return null;
        return {
          content: getComputedStyle(open).backgroundColor,
          arrowFill: getComputedStyle(arrow).fill,
          arrowBackground: getComputedStyle(arrow).backgroundColor,
        };
      });
      expect(colors).not.toBeNull();
      expect(colors?.arrowFill).toBe(colors?.content);
      expect(colors?.arrowBackground).toBe(colors?.content);
    }
  } finally {
    await fixture.close();
  }
});
