//! T100.1 D78：主模型 MiniMax-M3.1-Flash 覆盖成文字、看图链 MiniMax-M3，发图后自动转述。
//! 无 MINIMAX_API_KEY 时跳过；key 只从环境读，不落盘/日志。
import { expect, test } from '@playwright/test';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(300_000);

const MAIN = 'MiniMax-M3.1-Flash';
const VISION = 'MiniMax-M3';
const RED_PNG =
  'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAK0lEQVR4nO3NQQEAAATAQGTQP5kwSvC7BdjldMdn9XoHAAAAAAAAAAAAhy3gIwFE6inHLwAAAABJRU5ErkJggg==';

const input = (page: import('@playwright/test').Page) =>
  page.locator('[data-kind="composer-input"]');
const send = (page: import('@playwright/test').Page) => page.locator('[data-kind="send"]').click();

async function configure(
  request: import('@playwright/test').APIRequestContext,
  fixture: { baseUrl: string; token: string },
) {
  const settingsResponse = await apiPost(request, fixture, { method: 'model-settings' });
  const settings = (await settingsResponse.json()) as {
    settings: { connections: { id: string; endpoint: string; preset?: string }[] };
  };
  const connection = settings.settings.connections.find((item) => item.id === 'minimax-cn');
  if (!connection) throw new Error('D78 夹具里没有 minimax-cn 连接');
  const update = await apiPost(request, fixture, {
    method: 'connection-update',
    id: connection.id,
    endpoint: connection.endpoint,
    model: MAIN,
    preset: connection.preset,
    // 主模型输入只留 text；看图模型在目录里标了 image。
    models: [
      { id: MAIN, input: ['text'] },
      { id: VISION, input: ['text', 'image'] },
    ],
  });
  if (!update.ok()) throw new Error('connection-update ' + update.status());
  const select = await apiPost(request, fixture, {
    method: 'session-model-select',
    connection: connection.id,
    model: MAIN,
    effortWanted: 'max',
  });
  if (!select.ok()) throw new Error('session-model-select ' + select.status());
  const recheck = await apiPost(request, fixture, { method: 'recheck' });
  if (!recheck.ok()) throw new Error('recheck ' + recheck.status());
  const vision = await apiPost(request, fixture, {
    method: 'settings-update',
    visionModels: [{ connection: connection.id, model: VISION }],
  });
  if (!vision.ok()) throw new Error('settings-update ' + vision.status());
}

async function attachImage(page: import('@playwright/test').Page) {
  const transfer = await page.evaluateHandle((base64) => {
    const binary = atob(base64);
    const data = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) data[index] = binary.charCodeAt(index);
    const next = new DataTransfer();
    next.items.add(new File([data], 'red.png', { type: 'image/png' }));
    return next;
  }, RED_PNG);
  await page.locator('[data-kind="composer"]').dispatchEvent('drop', { dataTransfer: transfer });
  await expect(page.locator('[data-kind="attachment-image"]')).toBeVisible({ timeout: 20_000 });
}

async function waitFinished(page: import('@playwright/test').Page) {
  await expect(page.getByRole('button', { name: /已完成|停下了|已派出/ }).last()).toBeVisible({
    timeout: 180_000,
  });
}

test('D78 转述：主模型看不了图，看图链 MiniMax-M3 自动描述后回答红', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await configure(request, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(input(page)).toBeVisible({ timeout: 60_000 });
    await attachImage(page);
    await input(page).fill('这张图是什么颜色？只回一个颜色词。');
    await send(page);
    await waitFinished(page);
    const noteFiles = (await readdir(path.join(fixture.dataDir, 'sessions'))).filter((name) =>
      name.endsWith('.vision-notes.jsonl'),
    );
    const notes = await Promise.all(
      noteFiles.map((name) => readFile(path.join(fixture.dataDir, 'sessions', name), 'utf8')),
    );
    const joined = notes.join('');
    expect(joined).toContain('此前由视觉模型读取');
    expect(joined).not.toContain('没有给出描述');
    const answer = (
      await page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last().innerText()
    ).trim();
    expect(answer).toMatch(/红|red/i);
    // 会话日志里的原图仍是 Image 块；转述只改请求副本。
    const sessions = path.join(fixture.dataDir, 'sessions');
    const names = (await readdir(sessions)).filter((name) => name.endsWith('.jsonl'));
    let imageBlocks = 0;
    for (const name of names) {
      const text = await readFile(path.join(sessions, name), 'utf8');
      imageBlocks += (text.match(/"type":"image"/g) ?? []).length;
    }
    expect(imageBlocks).toBeGreaterThanOrEqual(1);
  } finally {
    await fixture.close();
  }
});
