//! T100.4 #11：安装卡进度/回执 sha256/设置页已装；本地假源，不连 HF。
import { expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(300_000);
const FIXTURE = path.join(process.cwd(), 'e2e/fixtures/ocr-mixed.png');

test('#11 安装卡点确认：本地假源进度、sha256 回执、设置页已装', async ({ page, request }) => {
  const files: Record<string, Buffer> = {
    'det.onnx': Buffer.from('fake-det'),
    'rec.onnx': Buffer.from('fake-rec'),
    'dict.txt': Buffer.from('fake-dict'),
  };
  const server = createServer(async (requestUrl, response) => {
    const name = (requestUrl.url ?? '').replace(/^\//, '');
    const body = files[name];
    if (!body) {
      response.statusCode = 404;
      response.end();
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
    response.setHeader('content-type', 'application/octet-stream');
    response.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  const dir = await mkdtemp(path.join(tmpdir(), 'aios-t100-install-'));
  const specFiles = Object.entries(files).map(([name, body]) => ({
    url: 'http://127.0.0.1:' + String(port) + '/' + name,
    sha256: createHash('sha256').update(body).digest('hex'),
    size: body.length,
    target: name,
    version: 'test@' + name,
  }));
  const manifest = [{
    id: 'pp-ocrv6-small',
    name: '本机 OCR 测试包',
    version: 'test-small@local',
    purpose: '测试本地假源安装',
    source: '本地假源（测试）',
    license: 'Apache-2.0',
    total_bytes: specFiles.reduce((sum, file) => sum + file.size, 0),
    files: specFiles,
  }];
  const manifestPath = path.join(dir, 'manifest.json');
  await writeFile(manifestPath, JSON.stringify(manifest));
  process.env.AIOS_PACKAGE_TEST_MANIFEST = manifestPath;

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
      models: [{ id: 'MiniMax-M3.1-Flash', input: ['text'] }],
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
    await input.fill('把这张图里的字读出来；本机读字没装就先装。');
    await page.locator('[data-kind="send"]').click();
    await expect(page.getByRole('button', { name: /已完成|停下了|已派出/ }).last()).toBeVisible({ timeout: 120_000 });
    const card = page.locator('[data-kind="confirm-card"]').last();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toContainText('pp-ocrv6-small');
    await card.getByRole('button', { name: '确认' }).click();
    await expect(card.locator('[data-kind="install-progress"]')).toBeVisible({ timeout: 20_000 });
    await expect(card).toContainText('sha256', { timeout: 60_000 });
    await expect(page.locator('[data-kind="confirm-card"]').last()).toContainText('3 个文件 sha256 都对上了');

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });
    await expect(section.locator('[data-kind="vision-ocr"]')).toHaveAttribute('data-phase', 'installed', { timeout: 30_000 });
    const packageRoot = path.join(fixture.dataDir, 'packages', 'pp-ocrv6-small', 'test-small-local');
    const listing = await readFile(path.join(packageRoot, manifest[0].files[0].target), 'utf8').catch(() => '');
    expect(listing).toBe('fake-det');
  } finally {
    delete process.env.AIOS_PACKAGE_TEST_MANIFEST;
    await fixture.close();
    server.close();
  }
});
