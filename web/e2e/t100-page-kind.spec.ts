//! T100.4 F1：设置页 Kind 三张确认卡（加看图模型/开关截桌面/装本机读字）与页面同动作。
import { expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(240_000);

test('F1 设置页 Kind 三张卡都走确认后和页面同一动作', async ({ page, request }) => {
  const files: Record<string, Buffer> = {
    'det.onnx': Buffer.from('kind-det'),
    'rec.onnx': Buffer.from('kind-rec'),
    'dict.txt': Buffer.from('kind-dict'),
  };
  const server = createServer((req, res) => {
    const name = (req.url ?? '').replace(/^\//, '');
    const body = files[name];
    if (!body) {
      res.statusCode = 404;
      res.end();
      return;
    }
    res.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  const dir = await mkdtemp(path.join(tmpdir(), 'aios-t100-kind-'));
  const manifest = [{
    id: 'pp-ocrv6-small',
    name: '本机 OCR 页 Kind 测试包',
    version: 'kind-test@local',
    purpose: '页 Kind 安装测试',
    source: '本地假源',
    license: 'Apache-2.0',
    total_bytes: Object.values(files).reduce((sum, body) => sum + body.length, 0),
    files: Object.entries(files).map(([name, body]) => ({
      url: 'http://127.0.0.1:' + String(port) + '/' + name,
      sha256: createHash('sha256').update(body).digest('hex'),
      size: body.length,
      target: name,
      version: 'kind@' + name,
    })),
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
      models: [
        { id: 'MiniMax-M3.1-Flash', input: ['text'] },
        { id: 'MiniMax-M3', input: ['text', 'image'] },
      ],
    });
    await apiPost(request, fixture, { method: 'recheck' });
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:model');
    const section = page.locator('[data-settings="vision"]');
    await expect(section).toBeVisible({ timeout: 60_000 });

    const openKind = async () => {
      await page.locator('[data-kind="page-kind-open"]').click();
      await expect(page.locator('[data-kind="page-kind"]')).toBeVisible();
    };
    const closeKind = async () => {
      await page.locator('[data-kind="page-kind"]').getByRole('button', { name: '关上' }).click();
      await expect(page.locator('[data-kind="page-kind"]')).toHaveCount(0);
    };
    const confirmCard = async (suggestion: string, cardTitle: string) => {
      await page.locator('[data-kind="page-kind"]').getByRole('button', { name: suggestion }).click();
      const card = page.locator('[data-kind="page-kind-card"]');
      await expect(card).toContainText(cardTitle);
      await card.getByRole('button', { name: /加为|打开|关掉|下载并安装/ }).click();
      await expect(page.locator('[data-kind="page-kind-receipt"], [data-kind="page-kind-progress"]').first()).toBeVisible({ timeout: 30_000 });
    };

    await openKind();
    await expect(page.locator('[data-kind="page-kind-suggestion"]')).toHaveCount(3);
    await confirmCard('加一个看图模型', '加一个看图模型');
    await closeKind();
    await expect(section.locator('[data-kind="vision-model"]')).toHaveCount(1);

    await openKind();
    await confirmCard('打开截桌面', '打开截桌面');
    await closeKind();
    await expect(section.locator('[data-kind="screen-toggle"]')).toHaveAttribute('data-state', 'checked');

    await openKind();
    await confirmCard('装本机读字', '装本机读字');
    await expect(page.locator('[data-kind="page-kind-card"]')).toContainText('sha256', { timeout: 30_000 });
    await closeKind();
    await expect(section.locator('[data-kind="vision-ocr"]')).toHaveAttribute('data-phase', 'installed', { timeout: 30_000 });
  } finally {
    delete process.env.AIOS_PACKAGE_TEST_MANIFEST;
    await fixture.close();
    server.close();
  }
});
