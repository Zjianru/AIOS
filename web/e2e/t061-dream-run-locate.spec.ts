import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * 做梦完成小卡的「看看」带 run id：记忆页定位并高亮这一次 run 的明细；
 * 两次做梦后点旧卡，落的是旧那次，不是最新。
 */

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

function chip(page: Page, kind: string) {
  return page.locator('[data-memo-kind="' + kind + '"]');
}

async function send(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

async function expandLastProcess(page: Page) {
  const process = page.locator('[data-kind="process"]').last();
  await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 20_000 });
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if ((await process.getAttribute('data-open')) !== 'true') {
      await process.locator('button').first().click();
    }
    const group = process.locator('[data-kind="process-group"] button').first();
    if ((await group.count()) > 0 && (await group.getAttribute('aria-expanded')) !== 'true') {
      await group.click();
    }
    const memoryChip = process.locator('[data-kind="memo-chip"]').first();
    if ((await memoryChip.count()) > 0 && (await memoryChip.isVisible())) return;
    await page.waitForTimeout(150);
  }
  throw new Error('工作过程展开后没看到记忆小卡');
}

async function sendAndExpand(page: Page, text: string) {
  const processes = page.locator('[data-kind="process"]');
  const before = await processes.count();
  await send(page, text);
  await expect(processes).toHaveCount(before + 1, { timeout: 20_000 });
  await expandLastProcess(page);
}

async function gotoMemory(page: Page) {
  const base = page.url().split('#')[0];
  await page.goto(base + '#memory');
  await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
}

async function gotoKind(page: Page) {
  const base = page.url().split('#')[0];
  await page.goto(base + '#kind');
}

test('新卡看看：定位并高亮最新一次做梦的明细', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(urlOf(fixture));
    await sendAndExpand(page, '记住：回答先给结论');
    await page.waitForTimeout(700);
    await sendAndExpand(page, '记一下你看到的编码问题');
    await page.waitForTimeout(700);
    for (const [text, answer] of [
      ['3', '好 3'],
      ['4', '好 4'],
      ['5', '好 5'],
    ]) {
      await send(page, text);
      await expect(page.getByText(answer, { exact: true })).toBeVisible({ timeout: 30_000 });
    }
    const permission = chip(page, 'permission').first();
    await expect(permission).toBeVisible({ timeout: 30_000 });
    await permission.locator('[data-action="memory-permit-allow"]').click();
    await expect(chip(page, 'dream_done')).toHaveCount(1, { timeout: 60_000 });

    await gotoMemory(page);
    await page.locator('[data-action="dream-now"]').click();
    await expect(page.locator('[data-kind="dream-run"]')).toHaveCount(2, { timeout: 60_000 });
    const latestId = await page.locator('[data-kind="dream-run"]').first().getAttribute('data-run-id');
    await gotoKind(page);
    await expect(chip(page, 'dream_done')).toHaveCount(2, { timeout: 60_000 });
    const newest = chip(page, 'dream_done').last();
    await expect(newest).toHaveAttribute('data-ref', latestId ?? '', { timeout: 20_000 });
    await newest.locator('[data-action="memory-dream-see"]').click();
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
    const highlighted = page.locator(
      '[data-kind="dream-run"][data-run-id="' + latestId + '"][data-highlight="true"]',
    );
    await expect(highlighted).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('旧卡看看：落的是旧那次，不是最新', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(urlOf(fixture));
    await sendAndExpand(page, '记住：回答先给结论');
    await page.waitForTimeout(700);
    await sendAndExpand(page, '记一下你看到的编码问题');
    await page.waitForTimeout(700);
    for (const [text, answer] of [
      ['3', '好 3'],
      ['4', '好 4'],
      ['5', '好 5'],
    ]) {
      await send(page, text);
      await expect(page.getByText(answer, { exact: true })).toBeVisible({ timeout: 30_000 });
    }
    const permission = chip(page, 'permission').first();
    await expect(permission).toBeVisible({ timeout: 30_000 });
    await permission.locator('[data-action="memory-permit-allow"]').click();
    await expect(chip(page, 'dream_done')).toHaveCount(1, { timeout: 60_000 });
    const oldId = await chip(page, 'dream_done').first().getAttribute('data-ref');

    await gotoMemory(page);
    await page.locator('[data-action="dream-now"]').click();
    await expect(page.locator('[data-kind="dream-run"]')).toHaveCount(2, { timeout: 60_000 });
    const latestId = await page.locator('[data-kind="dream-run"]').first().getAttribute('data-run-id');
    await gotoKind(page);
    await expect(chip(page, 'dream_done')).toHaveCount(2, { timeout: 60_000 });
    await chip(page, 'dream_done').first().locator('[data-action="memory-dream-see"]').click();
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
    await expect(
      page.locator(
        '[data-kind="dream-run"][data-run-id="' + oldId + '"][data-highlight="true"]',
      ),
    ).toBeVisible({ timeout: 20_000 });
    await expect(
      page.locator('[data-kind="dream-run"][data-run-id="' + latestId + '"]'),
    ).not.toHaveAttribute('data-highlight', 'true');
  } finally {
    await fixture.close();
  }
});
