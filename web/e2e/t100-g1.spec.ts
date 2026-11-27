//! T100.4 G1：主模型看不了图 → 卡提醒 → 设置加看图模型 → 回来看图。
import { expect, test } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(300_000);
const MAIN = 'MiniMax-M3.1-Flash';
const VISION = 'MiniMax-M3';
const FIXTURE = path.join(process.cwd(), 'e2e/fixtures/ocr-mixed.png');

test('G1 文字模型发图→卡提醒→设置加 M3→回来看图', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    const settingsResponse = await apiPost(request, fixture, { method: 'model-settings' });
    const settings = (await settingsResponse.json()) as {
      settings: { connections: { id: string; endpoint: string; preset?: string }[] };
    };
    const connection = settings.settings.connections.find((item) => item.id === 'minimax-cn')!;
    await apiPost(request, fixture, {
      method: 'connection-update',
      id: connection.id,
      endpoint: connection.endpoint,
      preset: connection.preset,
      model: MAIN,
      models: [{ id: MAIN, input: ['text'] }, { id: VISION, input: ['text', 'image'] }],
    });
    await apiPost(request, fixture, { method: 'recheck' });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const input = page.locator('[data-kind="composer-input"]');
    await expect(input).toBeVisible({ timeout: 60_000 });
    const base64 = (await readFile(FIXTURE)).toString('base64');
    const transfer = await page.evaluateHandle((encoded) => {
      const binary = atob(encoded);
      const data = new Uint8Array(binary.length);
      for (let index = 0; index < binary.length; index += 1) data[index] = binary.charCodeAt(index);
      const next = new DataTransfer();
      next.items.add(new File([data], 'ocr-mixed.png', { type: 'image/png' }));
      return next;
    }, base64);
    await page.locator('[data-kind="composer"]').dispatchEvent('drop', { dataTransfer: transfer });
    await expect(page.locator('[data-kind="attachment-image"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="no-vision"]')).toBeVisible();
    await input.fill('这张图里写了什么？');
    await page.locator('[data-kind="send"]').click();
    await expect(page.getByRole('button', { name: /已完成|停下了|已派出/ }).last()).toBeVisible({ timeout: 120_000 });
    const note = page.locator('[data-kind="image-omitted-note"]').last();
    await expect(note).toBeVisible({ timeout: 20_000 });
    const noteBefore = await note.textContent();

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await section.locator('[data-kind="vision-add"] select').selectOption('minimax-cn/MiniMax-M3');
    await section.getByRole('button', { name: '添加看图模型' }).click();
    await expect(section.locator('[data-kind="vision-model"]')).toHaveCount(1);

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const input2 = page.locator('[data-kind="composer-input"]');
    await expect(input2).toBeVisible({ timeout: 60_000 });
    await input2.fill('现在看看那张图，按附件 id 回头看。');
    await page.locator('[data-kind="send"]').click();
    await expect(page.locator('[data-turn]')).toHaveCount(2, { timeout: 30_000 });
    await expect(
      page.locator('[data-turn]').last().getByRole('button', { name: /已完成，用时|停下了|已派出/ }),
    ).toBeVisible({ timeout: 180_000 });
    const toolRows = page.locator('[data-kind="tool-images"]');
    await expect(toolRows.first()).toBeVisible({ timeout: 30_000 });
    await expect
      .poll(async () =>
        toolRows.evaluateAll((rows) =>
          rows.reduce((max, row) => Math.max(max, row.querySelectorAll('[data-kind="vision-image"]').length), 0),
        ),
      )
      .toBe(2);
    const presented = page.locator('[data-kind="presented-images"]').last();
    await expect(presented).toBeVisible({ timeout: 30_000 });
    await expect(presented.locator('[data-kind="vision-image"]')).toHaveCount(2);
    // 第一轮可能已经被虚拟列表滚出 DOM：滚回顶部再核历史那条。
    await page.locator('[data-turn]').first().scrollIntoViewIfNeeded();
    const noteAfter = page.locator('[data-kind="image-omitted-note"]').first();
    await expect(noteAfter).toHaveText(noteBefore ?? '', { timeout: 20_000 });
    await expect(page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last()).toContainText(/订单|A-1024|Total|1,280/);
  } finally {
    await fixture.close();
  }
});
