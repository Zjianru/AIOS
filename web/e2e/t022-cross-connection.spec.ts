import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function send(page: Page, text: string): Promise<void> {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

async function sessionLog(fixture: { dataDir: string }): Promise<{
  rows: { type: string; data?: Record<string, unknown> }[];
}> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  const lines = text.trim().split(String.fromCharCode(10)).map((line) => JSON.parse(line));
  return { rows: lines.slice(1) };
}

/// 跨连接换模型和并发 status 的端到端：status 正卡在汇率请求的 await 里时，
/// 确认卡选「另一连接里的模型」。窗口 A/B 的确定性复现在 service 集成测试；
/// 这里验页面这一路的最终事实：卡收成回执、选择器/刷新一致、status 与 settings 都是新模型。
test('跨连接换模型：status 刷新中确认，卡片/选择器/设置一致', async ({ page, request }) => {
  const fixture = await startKernelFixture({ t014Confirm: true, slowFx: true });
  try {
    // 卡上的第三项在另一条连接里：先存连接（和 t014-confirm 同样的准备）。
    const minimax = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M2.7-highspeed',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key-minimax',
      preset: 'minimax-cn',
      models: [{ id: 'MiniMax-M2.7-highspeed' }],
    });
    expect(minimax.ok()).toBeTruthy();
    // deepseek 改成「跟着厂商走」，卡上的另外两个 deepseek 模型才可选。
    const deepseek = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'deepseek',
      name: 'DeepSeek',
      model: 'deepseek-flash',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key',
      preset: 'deepseek',
      models: [],
    });
    expect(deepseek.ok()).toBeTruthy();

    await page.goto(urlOf(fixture));
    // 首屏 status 已经拉过一次汇率（代理会拒掉，服务走后备汇率）；等它到过代理后，
    // 再让下一次汇率请求被扣住——那正好是这一轮结束时的 status 刷新。
    await fixture.fx!.firstConnect;
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible();
    fixture.fx!.hold(true);

    await send(page, '把这条对话换成 V4 Pro 试试');
    const card = page.locator('[data-kind="confirm-card"]').first();
    await expect(card).toBeVisible({ timeout: 30_000 });
    const options = card.locator('[data-action="card-option"]');
    await expect(options).toHaveCount(3, { timeout: 30_000 });
    await expect(options.nth(2)).toContainText('MiniMax');

    // 这一轮结束后页面会刷 status；服务端已经进了 fx 的 await，这时才点确认。
    await fixture.fx!.held;
    await options.nth(2).click();
    const confirmButton = card.locator('[data-action="card-confirm"]');
    await confirmButton.click();
    await expect(confirmButton).toBeDisabled();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(card.locator('[data-kind="confirm-receipt"]')).toContainText(
      'MiniMax-M2.7-highspeed',
    );

    // 放行被扣住的 status：它拿的是换模型之前的快照，回包不能把选择器盖回旧模型。
    fixture.fx!.release();
    await expect(page.locator('[data-kind="model-picker"]')).toContainText(
      'MiniMax-M2.7-highspeed',
      { timeout: 20_000 },
    );

    // 服务端事实：status 的 connection/model 都是新模型，settings 的 lastSelection 也是。
    const statusResponse = await apiPost(request, fixture, { method: 'status' });
    expect(statusResponse.ok()).toBeTruthy();
    const status = (await statusResponse.json()) as {
      stats?: { connection?: string; model?: string };
    };
    expect(status.stats?.connection).toBe('minimax-cn');
    expect(status.stats?.model).toBe('MiniMax-M2.7-highspeed');
    const settings = JSON.parse(
      await readFile(path.join(fixture.dataDir, 'settings.json'), 'utf8'),
    ) as { lastSelection?: { connection?: string; model?: string } };
    expect(settings.lastSelection?.connection).toBe('minimax-cn');
    expect(settings.lastSelection?.model).toBe('MiniMax-M2.7-highspeed');

    // 会话日志里是 confirmed 的决定，选的是第三项、走 select_model。
    const log = await sessionLog(fixture);
    const decisions = log.rows.filter((row) => row.type === 'card/decision');
    expect(decisions).toHaveLength(1);
    expect((decisions[0].data as { phase?: string }).phase).toBe('confirmed');
    expect((decisions[0].data as { option?: string }).option).toBe('minimax-highspeed');
    expect((decisions[0].data as { action?: string }).action).toBe('select_model');

    // 刷新：卡片回执还在，选择器还是新模型。
    await page.reload();
    const reloaded = page.locator('[data-kind="confirm-card"]').first();
    await expect(reloaded).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(reloaded.locator('[data-kind="confirm-receipt"]')).toContainText(
      'MiniMax-M2.7-highspeed',
    );
    await expect(page.locator('[data-kind="model-picker"]')).toContainText(
      'MiniMax-M2.7-highspeed',
      { timeout: 20_000 },
    );
  } finally {
    await fixture.close();
  }
});
