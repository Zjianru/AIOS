import { test, expect, type Locator, type Page, type Route } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';
import {
  connectionRow, openVerifyDialog, selectVerifyModel, waitVerifyModels,
} from './verify-dialog';

/**
 * T034.2（spec v3）：连接验证走弹窗——保存连接后自动打开，也能从行上的「验证」打开。
 * - 列表由弹窗自己去 available-models 拉（连接 models/live/目录），**不预选**；
 *   没选模型时「验证」点不了，选了才发这一次请求。
 * - 成功是放大的绿对勾；失败是抖动的红叉 + 本次模型和可读原因。
 * - 这条路径只回报这一次：不写连接 check、不改聊天里的模型、不阻止发消息；
 *   保存连接后的旧自动验证（不带 model）照旧，不在本票改。
 */

const MINIMAX = 'minimax-cn';
const M31 = 'MiniMax-M3.1-Flash';

interface ChatModel {
  connection: string;
  model: string;
}

/**
 * 给夹具加一条 MiniMax 连接：跟厂商的列表走（models 空），存下的厂商列表把 M2 排在最前；
 * 连接当前模型是 M3.1（建连接时存下的）。
 */
async function seedMinimax(request: Parameters<typeof apiPost>[0], fixture: KernelFixture): Promise<void> {
  const saved = await apiPost(request, fixture, {
    method: 'connection-save',
    id: MINIMAX,
    name: 'MiniMax（国内站）',
    model: M31,
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
  await expect(picker).toContainText(M31, { timeout: 15_000 });
  await expect
    .poll(async () => (await chatModel(request, fixture)).model, { timeout: 15_000 })
    .toBe(M31);
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

/** 弹窗里的模型下拉：没有预选、按钮先置灰，选了才让点。 */
async function expectNoPreselection(dialog: Locator): Promise<void> {
  const select = dialog.locator('[data-verify="model-select"]');
  await expect(select).toHaveValue('');
  await expect(dialog.getByRole('button', { name: '验证', exact: true })).toBeDisabled();
}

test('保存连接后弹窗自动打开：列表是拉来的，没有默认选中', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t034Verify: true });
  try {
    await seedMinimax(request, fixture);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-kind="model-picker"]')).toBeVisible({ timeout: 30_000 });
    await openSettings(page, fixture);

    // 弹窗要自己去服务要列表：记下保存之后页面发的 available-models。
    const asked: string[] = [];
    page.on('request', (sent) => {
      const body = sent.postData() ?? '';
      if (body.includes('"method":"available-models"')) asked.push(body);
    });
    await connectionRow(page, MINIMAX).getByRole('button', { name: '编辑', exact: true }).click();
    await page.locator('#key-minimax-cn').fill('new-key');
    await page.locator('[data-settings="editor"]').getByRole('button', { name: '保存', exact: true }).click();

    // 保存成功后弹窗立刻出现，不用再点「验证」。
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-settings="editor"]')).toHaveCount(0);
    await waitVerifyModels(dialog);

    // 列表真的来自 available-models（连接 models/live/目录），而且只是「看一眼」，不落盘。
    expect(asked.join(' '), '保存后要问服务要这次能用的模型').toContain('"method":"available-models"');
    expect(asked.join(' '), '弹窗只是看一眼：不能把这次列表存成「新」的基准').toContain('"mode":"query"');
    const select = dialog.locator('[data-verify="model-select"]');
    const options = await select.locator('option').allTextContents();
    expect(options.join(' | ')).toContain(M31);
    expect(options.some((text) => text.trim() === 'MiniMax-M2'), '列表要有 M2').toBe(true);

    // 不默认 M3.1、不默认目录第一个：停在占位项，验证按钮点不了。
    await expectNoPreselection(dialog);
    await selectVerifyModel(dialog, M31);
    await expect(dialog.getByRole('button', { name: '验证', exact: true })).toBeEnabled();
  } finally {
    await fixture.close();
  }
});

test('选 M3.1 验证成功：绿勾放大，聊天模型和连接行都不变；「验证」还能再开同一个弹窗', async ({ page, request }) => {
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
    await expectNoPreselection(dialog);
    await selectVerifyModel(dialog, M31);
    const release = await holdVerifyRequest(page);
    await dialog.getByRole('button', { name: '验证', exact: true }).click();
    await expect(dialog.locator('[data-verify="checking"]')).toBeVisible();
    release();
    await expect(dialog.locator('[data-verify="ok"]')).toBeVisible({ timeout: 20_000 });
    await expect(dialog.locator('[data-verify="check"]')).toHaveCSS('animation-name', 'verify-check-pop');
    await expect(dialog.locator('[data-verify="model"]')).toContainText(M31);

    // 验证会签发一次凭证：设置页的签发记录要跟上，但连接 check 不动。
    await expect(page.locator('[data-settings="keyring"]')).toContainText('今天签发', { timeout: 20_000 });
    expect(await connectionCheck(request, fixture)).toEqual(checkBefore);
    await expect(connectionRow(page, MINIMAX)).toContainText('还没验证过');
    await expect(connectionRow(page, MINIMAX).locator('[data-card-state]')).toHaveAttribute('data-card-state', 'done');

    await dialog.getByRole('button', { name: '关闭', exact: true }).click();
    await expect(dialog).toHaveCount(0);

    // 行上的「验证」打开的是同一个弹窗：再开一次还是没有预选。
    const again = await openVerifyDialog(page, MINIMAX);
    await expectNoPreselection(again);
    await again.getByRole('button', { name: '取消', exact: true }).click();
    await expect(again).toHaveCount(0);

    expect(await chatModel(request, fixture)).toEqual(modelBefore);
    await backToKind(page);
    expect(await page.locator('[data-kind="model-picker"]').textContent()).toBe(pickerLabel);
  } finally {
    await fixture.close();
  }
});

test('选 M2 验证失败：红叉抖动 + 本次模型和原因，连接行和聊天模型都不变，照样能发消息', async ({ page, request }) => {
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
    await expectNoPreselection(dialog);
    await selectVerifyModel(dialog, 'MiniMax-M2');
    const release = await holdVerifyRequest(page);
    await dialog.getByRole('button', { name: '验证', exact: true }).click();
    await expect(dialog.locator('[data-verify="checking"]')).toBeVisible();
    release();
    await expect(dialog.locator('[data-verify="fail"]')).toBeVisible({ timeout: 20_000 });
    await expect(dialog.locator('[data-verify="cross"]')).toHaveCSS('animation-name', 'verify-cross-shake');
    await expect(dialog.locator('[data-verify="model"]')).toContainText('MiniMax-M2');
    await expect(dialog.locator('[data-verify="reason"]')).toContainText('model MiniMax-M2 not found');
    // T009 故事 3/4、测试 5：页面不显示内部码和状态数字；未知码给通用中文原因。
    await expect(dialog.locator('[data-verify="reason"]')).not.toContainText('HTTP_');
    await expect(dialog.locator('[data-verify="reason"]')).not.toContainText('404');
    await expect(dialog.locator('[data-verify="reason"]')).toContainText('返回了意外的错误');

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
