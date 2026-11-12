import { test, expect, type Page, type Route } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';
import { connectionRow, openVerifyDialog } from './verify-dialog';

/**
 * T034.2：连接的「验证」走弹窗——先选模型再验证。
 * - 模型列表按 连接勾的模型 → 厂商列表 → 目录；默认选连接当前的 model。
 * - 成功是放大的绿对勾；失败是抖动的红叉 + 本次模型和厂商原因。
 * - 这条路径只回报这一次：不写连接 check、不改聊天里的模型、不阻止发消息。
 */

const MINIMAX = 'minimax-cn';
const CHAT_MODEL = 'MiniMax-M3.1-Flash';

interface ChatModel {
  connection: string;
  model: string;
}

/**
 * 给夹具加一条 MiniMax 连接：跟厂商的列表走（models 空），存下的厂商列表把 M2 排在最前；
 * 连接当前模型是 M3.1（建连接时存下的）。弹窗默认该选 M3.1，而不是列表第一个 M2。
 */
async function seedMinimax(request: Parameters<typeof apiPost>[0], fixture: KernelFixture): Promise<void> {
  const saved = await apiPost(request, fixture, {
    method: 'connection-save',
    id: MINIMAX,
    name: 'MiniMax（国内站）',
    model: CHAT_MODEL,
    endpoint: fixture.fakeEndpoint,
    apiKey: 'test-key',
    preset: MINIMAX,
    models: [],
  });
  expect(saved.ok()).toBeTruthy();
}

async function chatModel(request: Parameters<typeof apiPost>[0], fixture: KernelFixture): Promise<ChatModel> {
  const response = await apiPost(request, fixture, { method: 'status' });
  const body = (await response.json()) as { stats?: { connection: string; model: string } };
  expect(body.stats, 'status 要有会话统计').toBeTruthy();
  return { connection: body.stats!.connection, model: body.stats!.model };
}

/** 在底栏把聊天模型切到 MiniMax 的 M3.1（服务端写会话选择），等 status 跟上。 */
async function selectChatModel(page: Page, request: Parameters<typeof apiPost>[0], fixture: KernelFixture): Promise<void> {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  const picker = page.locator('[data-kind="model-picker"]');
  await expect(picker).toBeVisible({ timeout: 30_000 });
  await picker.click();
  const item = page.getByRole('menuitem', { name: /MiniMax-M3\.1-Flash/ });
  await expect(item).toBeVisible({ timeout: 30_000 });
  await item.click({ force: true });
  await expect(picker).toContainText(CHAT_MODEL, { timeout: 15_000 });
  await expect
    .poll(async () => (await chatModel(request, fixture)).model, { timeout: 15_000 })
    .toBe(CHAT_MODEL);
}

async function openSettings(page: Page, fixture: KernelFixture): Promise<void> {
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
}

async function backToKind(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Kind', exact: true }).click();
  await expect(page.locator('[data-kind="model-picker"]')).toBeVisible({ timeout: 20_000 });
}

/** 扣住页面发给服务的 connection-verify，让 spinner 能被观察到；返回放行函数。 */
async function holdVerifyRequest(page: Page): Promise<() => void> {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(/\/api(\?|$)/, async (route: Route) => {
    const body = route.request().postData() ?? '';
    if (body.includes('connection-verify')) await gate;
    await route.continue();
  });
  return release;
}

async function connectionCheck(
  request: Parameters<typeof apiPost>[0],
  fixture: KernelFixture,
): Promise<{ kind: string; model: string }> {
  const response = await apiPost(request, fixture, { method: 'model-settings' });
  const body = (await response.json()) as {
    settings: { connections: { id: string; model: string; check: { kind: string } }[] };
  };
  const connection = body.settings.connections.find((item) => item.id === MINIMAX);
  expect(connection, 'MiniMax 连接要还在').toBeTruthy();
  return { kind: connection!.check.kind, model: connection!.model };
}

test('弹窗默认选连接当前的模型（不是模型列表第一个）', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t034Verify: true });
  try {
    await seedMinimax(request, fixture);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-kind="model-picker"]')).toBeVisible({ timeout: 30_000 });
    await openSettings(page, fixture);

    const dialog = await openVerifyDialog(page, MINIMAX);
    const select = dialog.locator('[data-verify="model-select"]');
    await expect(select).toHaveValue(CHAT_MODEL);
    const options = await select.locator('option').allTextContents();
    expect(options[0], '列表第一个是勾选的 M2').toContain('MiniMax-M2');
    expect(options.join(' ')).toContain(CHAT_MODEL);
  } finally {
    await fixture.close();
  }
});

test('验证成功：绿色对勾放大，连接行和聊天模型都不变', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t034Verify: true });
  try {
    await seedMinimax(request, fixture);
    await selectChatModel(page, request, fixture);
    const pickerLabel = await page.locator('[data-kind="model-picker"]').textContent();
    const modelBefore = await chatModel(request, fixture);
    const checkBefore = await connectionCheck(request, fixture);
    expect(checkBefore.kind).toBe('never');

    await openSettings(page, fixture);
    const dialog = await openVerifyDialog(page, MINIMAX);
    const release = await holdVerifyRequest(page);
    await dialog.getByRole('button', { name: '验证', exact: true }).click();
    await expect(dialog.locator('[data-verify="checking"]')).toBeVisible();
    release();
    await expect(dialog.locator('[data-verify="ok"]')).toBeVisible({ timeout: 20_000 });
    await expect(dialog.locator('[data-verify="check"]')).toHaveCSS('animation-name', 'verify-check-pop');
    await expect(dialog.locator('[data-verify="model"]')).toContainText(CHAT_MODEL);

    // 验证会签发一次凭证：设置页的签发记录要跟上，但连接 check 不动。
    await expect(page.locator('[data-settings="keyring"]')).toContainText('今天签发', { timeout: 20_000 });
    expect(await connectionCheck(request, fixture)).toEqual(checkBefore);
    await expect(connectionRow(page, MINIMAX)).toContainText('还没验证过');
    await expect(connectionRow(page, MINIMAX).locator('[data-card-state]')).toHaveAttribute('data-card-state', 'done');

    await dialog.getByRole('button', { name: '关闭', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(await chatModel(request, fixture)).toEqual(modelBefore);
    await backToKind(page);
    expect(await page.locator('[data-kind="model-picker"]').textContent()).toBe(pickerLabel);
  } finally {
    await fixture.close();
  }
});

test('验证失败：红叉抖动 + 本次模型和原因，连接行和聊天模型都不变，照样能发消息', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t034Verify: true });
  try {
    await seedMinimax(request, fixture);
    await selectChatModel(page, request, fixture);
    const pickerLabel = await page.locator('[data-kind="model-picker"]').textContent();
    const modelBefore = await chatModel(request, fixture);
    const checkBefore = await connectionCheck(request, fixture);
    expect(checkBefore.kind).toBe('never');

    await openSettings(page, fixture);
    const dialog = await openVerifyDialog(page, MINIMAX);
    await dialog.locator('[data-verify="model-select"]').selectOption('MiniMax-M2');
    const release = await holdVerifyRequest(page);
    await dialog.getByRole('button', { name: '验证', exact: true }).click();
    await expect(dialog.locator('[data-verify="checking"]')).toBeVisible();
    release();
    await expect(dialog.locator('[data-verify="fail"]')).toBeVisible({ timeout: 20_000 });
    await expect(dialog.locator('[data-verify="cross"]')).toHaveCSS('animation-name', 'verify-cross-shake');
    await expect(dialog.locator('[data-verify="model"]')).toContainText('MiniMax-M2');
    await expect(dialog.locator('[data-verify="reason"]')).toContainText('model MiniMax-M2 not found');

    expect(await connectionCheck(request, fixture)).toEqual(checkBefore);
    await expect(connectionRow(page, MINIMAX)).toContainText('还没验证过');
    await dialog.getByRole('button', { name: '关闭', exact: true }).click();
    await expect(dialog).toHaveCount(0);

    expect(await chatModel(request, fixture)).toEqual(modelBefore);
    await backToKind(page);
    expect(await page.locator('[data-kind="model-picker"]').textContent()).toBe(pickerLabel);
    await page.getByPlaceholder('和 Kind 说…').fill('验证失败了也不拦发消息。');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
