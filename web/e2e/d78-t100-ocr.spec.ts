//! T100.3 D78：本机 OCR 优先；中英混排逐字；本机成功不调看图链。
import { expect, test } from '@playwright/test';
import { readFile, readdir, mkdir, copyFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.skip(!process.env.AIOS_OCR_MODELS, '需要 AIOS_OCR_MODELS 指向已下载的 PP-OCRv6 模型');
test.setTimeout(300_000);
const MAIN = 'MiniMax-M3.1-Flash';
const VISION_TEXT_ONLY = 'MiniMax-M3.1-Flash';
const FIXTURE = path.join(process.cwd(), 'e2e/fixtures/ocr-mixed.png');

async function seedOcrPackage(dataDir: string) {
  const models = process.env.AIOS_OCR_MODELS!;
  const target = path.join(dataDir, 'packages', 'pp-ocrv6-small', 'v1');
  await mkdir(target, { recursive: true });
  for (const name of ['det.onnx', 'rec.onnx', 'dict.txt']) {
    await copyFile(path.join(models, name), path.join(target, name));
  }
}

test('D78 本机 OCR：中英混排逐字读，本机成功不调看图链', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await seedOcrPackage(fixture.dataDir);
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
      models: [
        { id: MAIN, input: ['text'] },
        { id: VISION_TEXT_ONLY, input: ['text'] },
      ],
    });
    await apiPost(request, fixture, { method: 'recheck' });
    // 故意给一个只认文字的链：本机 OCR 若没优先，这一轮就没有可用描述。
    await apiPost(request, fixture, {
      method: 'settings-update',
      visionModels: [{ connection: connection.id, model: VISION_TEXT_ONLY }],
    });
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
    await input.fill('这张图里的订单号和总额是什么？只回答图里的文字。');
    await page.locator('[data-kind="send"]').click();
    await expect(page.getByRole('button', { name: /已完成|停下了|已派出/ }).last()).toBeVisible({
      timeout: 180_000,
    });
    const answer = page.locator('[data-kind="assistant-answer"], [data-kind="answer"]');
    await expect(answer.last()).toContainText(/A-1024|1,280\.50|订单/, { timeout: 30_000 });
    const notesDir = path.join(fixture.dataDir, 'vision', 'notes');
    const noteNames = await readdir(notesDir).catch(() => [] as string[]);
    let noteText = '';
    for (const name of noteNames) {
      if (!name.endsWith('.json')) continue;
      noteText += await readFile(path.join(notesDir, name), 'utf8');
    }
    expect(noteText).toContain('local:pp-ocrv6-small');
  } finally {
    await fixture.close();
  }
});
