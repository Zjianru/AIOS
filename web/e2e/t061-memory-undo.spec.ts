import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * 记下了小卡的「撤销」= 删到隔离区（7 天内能恢复）：关于你里消失、隔离区能恢复；
 * 同一句原话能重新记入。彻底删只在记忆页。
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

async function expandProcess(page: Page) {
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
  await expandProcess(page);
}

async function gotoMemory(page: Page) {
  const base = page.url().split('#')[0];
  await page.goto(base + '#memory');
  await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
}

/** 记住一句，点小卡撤销，等回执说明可恢复。 */
async function rememberAndUndo(page: Page) {
  await sendAndExpand(page, '记住：回答先给结论');
  const rememberChip = chip(page, 'remember').first();
  await expect(rememberChip).toBeVisible({ timeout: 30_000 });
  // 小卡上只有可恢复的撤销，没有彻底删。
  await expect(rememberChip.locator('[data-action="memory-delete"]')).toBeVisible();
  await expect(rememberChip.locator('[data-action="memory-forget"]')).toHaveCount(0);
  await rememberChip.locator('[data-action="memory-delete"]').click();
  await expect(page.getByText('7 天内能恢复', { exact: false })).toBeVisible({ timeout: 20_000 });
  // 撤销后小卡状态跟上：文案变已撤销，改/撤销按钮消失（不能拿隔离条目再点 Delete）。
  await expect(rememberChip).toContainText('已撤销', { timeout: 20_000 });
  await expect(rememberChip.locator('[data-action="memory-delete"]')).toHaveCount(0);
  await expect(rememberChip.locator('[data-action="memory-edit"]')).toHaveCount(0);
}

test('记下了小卡的撤销进隔离区：关于你里消失，隔离区能找回', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(urlOf(fixture));
    await rememberAndUndo(page);
    await gotoMemory(page);
    await expect(page.locator('[data-kind="memory-about"]')).not.toContainText(
      '回答先给结论',
      { timeout: 20_000 },
    );
    await expect(page.locator('[data-kind="memory-quarantine"]')).toContainText(
      '回答先给结论',
      { timeout: 20_000 },
    );
    // 隔离区里有恢复按钮；彻底删只在记忆页。
    const quarantined = page
      .locator('[data-kind="memory-quarantine-item"]')
      .filter({ hasText: '回答先给结论' });
    await expect(quarantined.locator('[data-action="memory-restore"]')).toBeVisible();
    await expect(quarantined.locator('[data-action="memory-forget"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('隔离区恢复后回到关于你', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(urlOf(fixture));
    await rememberAndUndo(page);
    await gotoMemory(page);
    const quarantined = page
      .locator('[data-kind="memory-quarantine-item"]')
      .filter({ hasText: '回答先给结论' });
    await expect(quarantined).toBeVisible({ timeout: 20_000 });
    await quarantined.locator('[data-action="memory-restore"]').click();
    await expect(page.locator('[data-kind="memory-about"]')).toContainText(
      '回答先给结论',
      { timeout: 20_000 },
    );
  } finally {
    await fixture.close();
  }
});

test('同一句原话能再记入（撤销不是彻底删）', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryAgain: true });
  try {
    await page.goto(urlOf(fixture));
    await rememberAndUndo(page);
    // 同一句原话再说一次：能重新记入，不出现「这个来源已经忘记过」的拒绝。
    await sendAndExpand(page, '记住：回答先给结论');
    await expect(chip(page, 'remember')).toHaveCount(2, { timeout: 30_000 });
    const fresh = chip(page, 'remember').filter({ hasText: '回答先给结论' }).last();
    await expect(fresh).toContainText('记下了', { timeout: 20_000 });
    await expect(fresh).not.toContainText('已撤销');
    await gotoMemory(page);
    await expect(page.locator('[data-kind="memory-about"]')).toContainText(
      '回答先给结论',
      { timeout: 20_000 },
    );
  } finally {
    await fixture.close();
  }
});
