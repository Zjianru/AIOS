//! T093.5 D78：真 key（MiniMax）输入模态两条路。
//! 无 key 时跳过，由 D78 环境跑（key 从 ~/.zshrc 运行时取，不落文件/命令行/证据）。
import { expect, test } from '@playwright/test';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(300_000);

const MODEL = 'MiniMax-M3.1-Flash';
const RED_PNG =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

const input = (page: import('@playwright/test').Page) =>
  page.locator('[data-kind="composer-input"]');
const send = (page: import('@playwright/test').Page) => page.locator('[data-kind="send"]').click();

async function overrideInput(
  request: import('@playwright/test').APIRequestContext,
  fixture: { baseUrl: string; token: string },
  inputModalities: string[],
) {
  const settingsResponse = await apiPost(request, fixture, { method: 'model-settings' });
  const settings = (await settingsResponse.json()) as {
    settings: { connections: { id: string; endpoint: string; preset?: string }[] };
  };
  const connection = settings.settings.connections.find((item) => item.id === 'minimax-cn');
  if (!connection) throw new Error('D78 夹具里没有 minimax-cn 连接');
  await apiPost(request, fixture, {
    method: 'connection-update',
    id: connection.id,
    endpoint: connection.endpoint,
    model: MODEL,
    preset: connection.preset,
    models: [
      inputModalities.length > 0 ? { id: MODEL, input: inputModalities } : { id: MODEL },
    ],
  });
  await apiPost(request, fixture, {
    method: 'session-model-select',
    connection: connection.id,
    model: MODEL,
    effortWanted: 'max',
  });
  // 让根会话按新设置重开一次，模型工厂才会重新解析 supports_images。
  await apiPost(request, fixture, { method: 'recheck' });
}

async function attachRedImage(page: import('@playwright/test').Page) {
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

test('D78 输入模态：MiniMax 覆盖成文字时发图照实说看不了', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await overrideInput(request, fixture, ['text']);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(input(page)).toBeVisible({ timeout: 60_000 });
    await attachRedImage(page);
    await input(page).fill('这张图是什么颜色？只回一个颜色词。');
    await send(page);
    await waitFinished(page);
    const answer = (await page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last().innerText()).trim();
    expect(answer).toMatch(/看不了|看不到|看不见|没能看|不支持图片|无法看|不能看|没法/);
    expect(answer).not.toMatch(/红色|red/i);
  } finally {
    await fixture.close();
  }
});

test('D78 输入模态：跟目录（MiniMax 标了 image）时发图能正常看图回答', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await overrideInput(request, fixture, []);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(input(page)).toBeVisible({ timeout: 60_000 });
    await attachRedImage(page);
    await input(page).fill('这张图是什么颜色？只回一个颜色词。');
    await send(page);
    await waitFinished(page);
    const answer = (await page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last().innerText()).trim();
    expect(answer).toMatch(/红|粉|玫|red|pink/i);
  } finally {
    await fixture.close();
  }
});
