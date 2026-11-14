import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T056.1：门判完的占板 + 组接受补 Kind 卡回执。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.locator('[data-kind="composer-input"]').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

const column = (page: Page, key: string) =>
  page.locator('[data-kind="work-board"] section[data-column="' + key + '"]');

async function gatePassedWith(page: Page, fixture: { baseUrl: string; token: string }) {
  await page.goto(urlOf(fixture));
  await sendKind(
    page,
    '先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果',
  );
  const confirm = page.locator('[data-kind="confirm-card"]').first();
  await expect(confirm).toBeVisible({ timeout: 60_000 });
  await confirm.locator('[data-action="card-option"]').first().click();
  await confirm.locator('[data-action="card-confirm"]').click();
  await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
  const gateCard = page
    .locator('[data-kind="task-result"]')
    .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '独立核查' }) })
    .first();
  await expect(gateCard.locator('[data-kind="gate-pass"]')).toBeVisible({ timeout: 60_000 });
  await gateCard.locator('[data-kind="gate-pass"]').click();
  await expect(gateCard.locator('[data-kind="gate-verdict"]')).toContainText('你判了通过', {
    timeout: 40_000,
  });
}

test('门判完、接受前：门任务在「在做」列，词「已判」', async ({ page }) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  try {
    await page.goto(urlOf(fixture));
    await gatePassedWith(page, fixture);
    await page.goto(urlOf(fixture, '#work'));
    await expect(column(page, 'doing')).toContainText('独立核查', { timeout: 30_000 });
    const tile = column(page, 'doing')
      .locator('[data-kind="work-card"]')
      .filter({ hasText: '独立核查' })
      .first();
    await expect(tile).toBeVisible();
    await expect(tile.locator('[data-kind="work-word"]')).toHaveText('已判');
  } finally {
    await fixture.close();
  }
});

test('「等你」组接受后回 Kind：结果卡写「你接受了」，不再出接受按钮', async ({
  page,
}) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t053Gate: true });
  try {
    await page.goto(urlOf(fixture));
    await gatePassedWith(page, fixture);
    await page.goto(urlOf(fixture, '#work'));
    const waiting = page.locator('[data-pane="waiting"]');
    const accept = waiting
      .locator('[data-kind="wait-item"][data-wait-kind="accept"]')
      .filter({ hasText: '先查行数' })
      .first();
    await expect(accept).toBeVisible({ timeout: 30_000 });
    await accept.locator('[data-action="wait-accept"]').click();
    await expect(waiting.locator('[data-kind="accept-receipt"]')).toContainText('接受了 1 项', {
      timeout: 30_000,
    });
    await page.goto(urlOf(fixture, '#kind'));
    const guardedCard = page
      .locator('[data-kind="task-result"]')
      .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '先查行数' }) })
      .first();
    await expect(guardedCard.locator('[data-kind="task-accepted"]')).toContainText('你接受了', {
      timeout: 30_000,
    });
    await expect(guardedCard.locator('[data-kind="task-accept"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
