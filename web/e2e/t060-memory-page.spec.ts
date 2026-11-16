import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * 记忆页联动：对话里的小卡（记下了/Kind 记了一笔/做梦许可/整理完了）、
 * 记忆页的关于你/条目/日记/做梦/隔离区、按 ref 读状态与撤销。
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

/** 发一句并等这一轮的过程出现，再展开到能看见工具小卡。 */
async function sendAndExpand(page: Page, text: string) {
  const processes = page.locator('[data-kind="process"]');
  const before = await processes.count();
  await send(page, text);
  await expect(processes).toHaveCount(before + 1, { timeout: 20_000 });
  await expandProcess(page);
}

async function expandProcess(page: Page) {
  const process = page.locator('[data-kind="process"]').last();
  // 回合还在跑时过程是自动展开的；点之前先看状态，别把展开点成收起。
  await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 20_000 });
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if ((await process.getAttribute('data-open')) !== 'true') {
      await process.locator('button').first().click();
    }
    const group = process.locator('[data-kind="process-group"] button').first();
    if ((await group.count()) > 0 && (await group.getAttribute('aria-expanded')) !== 'true') {
      await group.click();
    }
    const chip = process.locator('[data-kind="memo-chip"]').first();
    if ((await chip.count()) > 0 && (await chip.isVisible())) return;
    await page.waitForTimeout(150);
  }
  throw new Error('工作过程展开后没看到记忆小卡');
}

async function gotoMemory(page: Page) {
  const base = page.url().split('#')[0];
  await page.goto(base + '#memory');
}

test('记忆：记住/记一笔的小卡，攒够许可→整理→撤销，记忆页读改删恢复', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(urlOf(fixture));

    // 故事 1：用户说记住 → 一行小卡（和回复分开），有改/撤销。
    await sendAndExpand(page, '记住：回答先给结论');
    const rememberChip = chip(page, 'remember').first();
    await expect(rememberChip).toBeVisible({ timeout: 30_000 });
    await expect(rememberChip).toContainText('记下了');
    await expect(rememberChip).toContainText('回答先给结论');
    await expect(rememberChip.locator('[data-action="memory-edit"]')).toBeVisible();
    await expect(rememberChip.locator('[data-action="memory-delete"]')).toBeVisible();
    await expect(page.locator('[data-kind="user-bubble"] [data-kind="memo-chip"]')).toHaveCount(0);
    await expect(
      page.locator('[data-kind="assistant-answer"] [data-kind="memo-chip"]'),
    ).toHaveCount(0);

    // 故事 3：Kind 记一笔 → 笔记小卡，有看/删。
    await sendAndExpand(page, '记一下你看到的编码问题');
    const noteChip = chip(page, 'note').first();
    await expect(noteChip).toBeVisible({ timeout: 30_000 });
    await expect(noteChip).toContainText('Kind 记了一笔');
    await expect(noteChip).toContainText('GBK CSV');
    await expect(noteChip.locator('[data-action="memory-see"]')).toBeVisible();
    await expect(noteChip.locator('[data-action="memory-delete"]')).toBeVisible();

    // 故事 6：再攒几轮，够 5 个回合出许可卡；允许前不跑。
    // 一轮没做完时发送会被挡（Kind 还在做上一轮），所以逐轮等到回答再发下一句。
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
    await expect(permission).toContainText('做梦许可');
    await expect(permission.locator('[data-action="memory-permit-allow"]')).toBeVisible();
    await expect(permission.locator('[data-action="memory-permit-later"]')).toBeVisible();

    // 故事 7：允许 → 空闲时跑一次 → 整理完小卡（看看/全部撤销）。
    await permission.locator('[data-action="memory-permit-allow"]').click();
    const done = chip(page, 'dream_done').first();
    await expect(done).toBeVisible({ timeout: 60_000 });
    await expect(done).toContainText('整理完了');
    await expect(done.locator('[data-action="memory-dream-see"]')).toBeVisible();
    await expect(done.locator('[data-action="memory-dream-undo"]')).toBeVisible();
    const runRef = await done.getAttribute('data-ref');
    expect(runRef ?? '').toContain('dream-');

    // 全部撤销 → 卡片状态按 ref 从库读成已撤销。
    await done.locator('[data-action="memory-dream-undo"]').click();
    await expect(chip(page, 'dream_done').first()).toContainText('已撤销', { timeout: 20_000 });

    // 故事 12：记忆页的分区都在；关于你有记住的那条，条目里 Kind 记的有一条。
    await gotoMemory(page);
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
    const about = page.locator('[data-kind="memory-about"]');
    await expect(about).toContainText('回答先给结论');
    await expect(page.locator('[data-kind="memory-group"][data-group="note"]')).toContainText(
      'GBK CSV',
    );
    await expect(page.locator('[data-kind="memory-diary"]')).toBeVisible();
    await expect(page.locator('[data-kind="memory-dream"]')).toBeVisible();
    await expect(page.locator('[data-kind="memory-quarantine"]')).toBeVisible();
    await expect(page.locator('[data-kind="dream-runs"] [data-kind="dream-run"]')).toContainText(
      '整理完了',
    );
    await expect(page.locator('[data-kind="dream-runs"] [data-kind="dream-run"]')).toHaveCount(1);

    // 历次明细：再来一次整理（这次是合并已有笔记），撤销后这一处显示已撤销、正文回到原样。
    await page.locator('[data-action="dream-now"]').click();
    await expect(page.locator('[data-kind="dream-runs"] [data-kind="dream-run"]')).toHaveCount(
      2,
      { timeout: 60_000 },
    );
    const newest = page.locator('[data-kind="dream-runs"] [data-kind="dream-run"]').first();
    await expect(newest).toContainText('整理完了', { timeout: 60_000 });
    await expect(newest).toContainText('合并');
    await newest.locator('[data-action="dream-undo"]').click();
    await expect(newest.locator('[data-kind="dream-change-undone"]').first()).toContainText(
      '已撤销',
      { timeout: 30_000 },
    );
    await expect(page.locator('[data-kind="memory-group"][data-group="note"]')).toContainText(
      'sources/ 下有 GBK 的 CSV，读之前先探编码',
    );

    // 故事 11：删笔记 → 隔离区 → 恢复；再删 → 彻底删 → 搜不到了。
    const noteRow = page
      .locator('[data-kind="memory-group"][data-group="note"] [data-kind="memory-item"]')
      .filter({ hasText: 'GBK CSV' });
    await noteRow.locator('[data-action="memory-delete"]').click();
    const quarantine = page.locator('[data-kind="memory-quarantine"]');
    await expect(quarantine).toContainText('GBK CSV', { timeout: 15_000 });
    await quarantine
      .locator('[data-kind="memory-quarantine-item"]')
      .filter({ hasText: 'GBK CSV' })
      .locator('[data-action="memory-restore"]')
      .click();
    await expect(
      page.locator('[data-kind="memory-group"][data-group="note"]'),
    ).toContainText('GBK CSV', { timeout: 15_000 });
    const noteRowAgain = page
      .locator('[data-kind="memory-group"][data-group="note"] [data-kind="memory-item"]')
      .filter({ hasText: 'GBK CSV' });
    await noteRowAgain.locator('[data-action="memory-delete"]').click();
    await expect(quarantine).toContainText('GBK CSV', { timeout: 15_000 });
    await quarantine
      .locator('[data-kind="memory-quarantine-item"]')
      .filter({ hasText: 'GBK CSV' })
      .locator('[data-action="memory-forget"]')
      .click();
    await page.locator('[data-kind="memory-search-input"]').fill('GBK CSV');
    await page.locator('[data-action="memory-search"]').click();
    await expect(page.locator('[data-kind="memory-search-results"]')).not.toContainText(
      'GBK CSV',
      { timeout: 15_000 },
    );

    // 故事 10（记忆页更正出新版本）：改关于你的正文并保存。
    const aboutRow = page
      .locator('[data-kind="memory-about"] [data-kind="memory-item"]')
      .filter({ hasText: '回答先给结论' });
    await aboutRow.locator('[data-action="memory-correct"]').click();
    const editor = page.locator('[data-kind="memory-editor"]');
    await expect(editor).toBeVisible();
    await editor.locator('[data-kind="memory-editor-body"]').fill('先给结论，再给理由和取舍');
    await editor.locator('[data-action="memory-save"]').click();
    await expect(page.locator('[data-kind="memory-about"]')).toContainText(
      '先给结论，再给理由和取舍',
      { timeout: 15_000 },
    );

    // 故事 10：历次版本写清谁改的（这次是用户改的）。
    await aboutRow.locator('[data-action="memory-correct"]').click();
    const editorAgain = page.locator('[data-kind="memory-editor"]');
    await expect(editorAgain).toBeVisible();
    await expect(editorAgain).toContainText('你改的', { timeout: 15_000 });
    await editorAgain.locator('[data-action="memory-cancel"]').click();

    // 刷新：页面从库重读，仍是改后的版本。
    await page.reload();
    await expect(page.locator('[data-kind="memory-page"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="memory-about"]')).toContainText(
      '先给结论，再给理由和取舍',
    );
    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test('没跑成的小卡写一行原因（故事 8）', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true, memoryFailDream: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '记住：回答先给结论');
    await page.waitForTimeout(900);
    await send(page, '记一下你看到的编码问题');
    await page.waitForTimeout(900);
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
    const failed = chip(page, 'dream_failed').first();
    await expect(failed).toBeVisible({ timeout: 60_000 });
    await expect(failed).toContainText('整理没跑成');
    const reason = failed.locator('[data-kind="memo-error"]');
    await expect(reason).toBeVisible({ timeout: 20_000 });
    await expect(reason).not.toHaveText('');
  } finally {
    await fixture.close();
  }
});


test('许可卡在 turn 中途到、turn 结束后还在，允许可点（live 渲染）', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true, memoryLiveCard: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '记住：回答先给结论');
    await page.waitForTimeout(900);
    await send(page, '记一下你看到的编码问题');
    await page.waitForTimeout(900);
    for (const [text, answer] of [
      ['3', '好 3'],
      ['4', '好 4'],
    ]) {
      await send(page, text);
      await expect(page.getByText(answer, { exact: true })).toBeVisible({ timeout: 30_000 });
    }
    // 第五轮 Kind 先加载 aios-memory，再调 memory_dream_now：许可卡在 turn 中途写进
    // 日志，turn 结束后必须还在。
    await send(page, '5');
    await expect(page.getByText('好 5', { exact: true })).toBeVisible({ timeout: 30_000 });
    const permission = chip(page, 'permission').first();
    await expect(permission).toBeVisible({ timeout: 30_000 });
    await expect(permission).toContainText('做梦许可');
    // 点允许：dream 真的跑，整理完的小卡出现。
    await permission.locator('[data-action="memory-permit-allow"]').click();
    const done = chip(page, 'dream_done').first();
    await expect(done).toBeVisible({ timeout: 60_000 });
    await expect(done).toContainText('整理完了');
  } finally {
    await fixture.close();
  }
});

test('搜索命中可点：记忆条目高亮，对话回那一轮（故事 4）', async ({ page }) => {
  const fixture = await startKernelFixture({ memoryFlow: true });
  try {
    await page.goto(urlOf(fixture));
    await send(page, '记住：回答先给结论');
    await page.waitForTimeout(900);
    await gotoMemory(page);

    const search = page.locator('[data-kind="memory-search-input"]');
    await search.fill('结论');
    await search.press('Enter');
    const memoryHit = page.locator('[data-kind="memory-hit"][data-hit-kind="memory"]').first();
    await expect(memoryHit).toBeVisible({ timeout: 20_000 });
    await memoryHit.click();
    const highlighted = page.locator('[data-kind="memory-item"][data-highlight="true"]').first();
    await expect(highlighted).toBeVisible({ timeout: 20_000 });
    await expect(highlighted).toContainText('回答先给结论');

    await search.fill('记住');
    await search.press('Enter');
    const conversationHit = page
      .locator('[data-kind="memory-hit"][data-hit-kind="conversation"]')
      .first();
    await expect(conversationHit).toBeVisible({ timeout: 20_000 });
    await conversationHit.click();
    await expect(page).toHaveURL(/#kind/, { timeout: 20_000 });
    await expect(page.getByText('记住：回答先给结论', { exact: false })).toBeVisible({
      timeout: 30_000,
    });
  } finally {
    await fixture.close();
  }
});


