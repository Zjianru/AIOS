//! T100.4 F2：同一附件先 omission→填看图链→重发，第一条历史 note 不变。
import { expect, test } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(240_000);
const FIXTURE = path.join(process.cwd(), 'e2e/fixtures/ocr-mixed.png');

async function drop(page: any, name: string) {
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
  await expect(page.locator('[data-kind="attachment-image"]')).toHaveCount(1, { timeout: 20_000 });
}

async function send(page: any, text: string) {
  const input = page.locator('[data-kind="composer-input"]');
  await input.fill(text);
  const turns = page.locator('[data-turn]');
  const before = await turns.count();
  await page.locator('[data-kind="send"]').click();
  await expect.poll(async () => turns.count(), { timeout: 30_000 }).toBeGreaterThan(before);
  await expect
    .poll(async () => {
      const last = turns.last();
      return last.getByRole('button', { name: /已完成，用时|停下了|已派出/ }).count();
    }, { timeout: 180_000 })
    .toBeGreaterThan(0);
}

test('F2 同一附件重发不覆盖历史 note', async ({ page, request }) => {
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
      model: 'MiniMax-M3.1-Flash',
      models: [
        { id: 'MiniMax-M3.1-Flash', input: ['text'] },
        { id: 'MiniMax-M3', input: ['text', 'image'] },
      ],
    });
    await apiPost(request, fixture, { method: 'recheck' });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
    await drop(page, 'first');
    await send(page, '第一次发这张图');
    const firstNote = page.locator('[data-kind="image-omitted-note"]').first();
    await expect(firstNote).toBeVisible({ timeout: 20_000 });
    const beforeText = await firstNote.textContent();

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await section.locator('[data-kind="vision-add"] select').selectOption('minimax-cn/MiniMax-M3');
    await section.getByRole('button', { name: '添加看图模型' }).click();
    await expect(section.locator('[data-kind="vision-model"]')).toHaveCount(1);

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 60_000 });
    await drop(page, 'second');
    await send(page, '同一张图再发一次');

    await page
      .locator('[data-pane="conversation"] [data-virtuoso-scroller="true"]')
      .evaluate((element) => {
        element.scrollTop = 0;
      });
    await page.waitForTimeout(500);
    const historyNote = page.locator('[data-kind="image-omitted-note"]').first();
    await expect(historyNote).toHaveText(beforeText ?? '');
    await expect(page.locator('[data-kind="image-paraphrased-note"]')).toHaveCount(1);
  } finally {
    await fixture.close();
  }
});
