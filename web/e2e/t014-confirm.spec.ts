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

/// 确认卡：三个选项的卡选一项确认收成回执、刷新还在、模型选择器换了；另一张卡点暂不。
test('确认卡：选一项确认、刷新还在、模型选择器跟着换；另一张卡暂不', async ({
    page,
    request,
  }) => {
  const fixture = await startKernelFixture({ t014Confirm: true });
  try {
    // 第三项指向 MiniMax 高速版：先存一个只有这个模型的连接，服务端才认得这个选项。
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
    // 夹具的连接只勾了一个模型；重存成「跟着厂商走」，可选模型才来自目录
    // （设置页上的连接就是这样存的）。这是测试准备，不走产品的新路径。
    await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'deepseek',
      name: 'DeepSeek',
      model: 'deepseek-flash',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key',
      preset: 'deepseek',
      models: [],
    });
    // 把 card-act 的响应延迟一点：点完按钮到回执之间有一段可观察的等待。
    let cardActs = 0;
    await page.route('**/api', async (route) => {
      const body = route.request().postDataJSON() as { method?: string } | null;
      if (body?.method === 'card-act') {
        cardActs += 1;
        await new Promise((resolve) => setTimeout(resolve, 400));
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture));
    await send(page, '把这条对话换成 V4 Pro 试试');

    const first = page.locator('[data-kind="confirm-card"]').first();
    await expect(first).toBeVisible({ timeout: 30_000 });
    await expect(first).toHaveAttribute('data-state', 'pending');
    const options = first.locator('[data-action="card-option"]');
    // 三项都在；第三项 label 是 22 字的 MiniMax 高速版，超长只截断显示、不丢选项。
    await expect(options).toHaveCount(3);
    await expect(options.nth(2).locator('[data-kind="confirm-option-label"]')).toHaveText(
      'MiniMax-M2.7-highspe',
    );
    // 每个选项下面有服务端填的事实行；换模型有模型行，模型和强度都换时两行。
    await expect(options.nth(0).locator('[data-kind="confirm-fact"]')).toHaveCount(1);
    await expect(options.nth(0)).toContainText('模型');
    await expect(options.nth(0)).toContainText('→');
    await expect(options.nth(1).locator('[data-kind="confirm-fact"]')).toHaveCount(2);
    await expect(options.nth(1)).toContainText('推理强度');
    await expect(options.nth(2).locator('[data-kind="confirm-fact"]')).toHaveCount(1);
    await expect(options.nth(2)).toContainText('模型');
    await expect(options.nth(2)).not.toContainText('推理强度');

    await options.nth(1).click();
    const confirmButton = first.locator('[data-action="card-confirm"]');
    // 双击：第一下之后按钮就置灰，第二下不该再发请求；回执到之前一直是 pending。
    await confirmButton.dblclick();
    await expect(confirmButton).toBeDisabled();
    await expect(confirmButton).toHaveText('处理中…');
    await expect(first).toHaveAttribute('data-state', 'pending');
    await expect(first).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    expect(cardActs).toBe(1);
    await expect(first.locator('[data-kind="confirm-receipt"]')).toContainText('已执行');
    await expect(first.locator('[data-kind="confirm-receipt"]')).toContainText('V4 Pro · 最强思考');

    // 模型选择器显示新模型（选择来自日志里的 session/model）。
    // 显示名来自页面目录（DeepSeek-V4-Pro）或服务目录（DeepSeek V4 Pro），都算新模型。
    await expect(page.locator('[data-kind="model-picker"]')).toContainText(/DeepSeek[- ]V4[- ]Pro/, {
      timeout: 20_000,
    });

    // 刷新页面：回执还在，模型选择器也还是新模型。
    await page.reload();
    const reloaded = page.locator('[data-kind="confirm-card"]').first();
    await expect(reloaded).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(reloaded.locator('[data-kind="confirm-receipt"]')).toContainText('已执行');
    await expect(page.locator('[data-kind="model-picker"]')).toContainText(/DeepSeek[- ]V4[- ]Pro/);

    // 第二轮再出一张卡，点暂不：什么都不执行，收成回执。
    await send(page, '再给我看看别的');
    await expect(page.locator('[data-kind="confirm-card"]')).toHaveCount(2, { timeout: 30_000 });
    const second = page.locator('[data-kind="confirm-card"]').last();
    await expect(second).toHaveAttribute('data-state', 'pending');
    // 模型没变、只换强度：只有推理强度一行，未变的模型行不显示。
    const secondOption = second.locator('[data-action="card-option"]');
    await expect(secondOption).toHaveCount(1);
    const secondFact = secondOption.locator('[data-kind="confirm-fact"]');
    await expect(secondFact).toHaveCount(1);
    await expect(secondFact).toContainText('推理强度');
    await expect(secondFact).not.toContainText('模型');
    await second.getByRole('button', { name: '暂不', exact: true }).click();
    await expect(second).toHaveAttribute('data-state', 'done', { timeout: 30_000 });
    await expect(second.locator('[data-kind="confirm-receipt"]')).toContainText('暂不');

    const log = await sessionLog(fixture);
    const decisions = log.rows.filter((row) => row.type === 'card/decision');
    expect(decisions.map((row) => (row.data as { phase: string }).phase)).toEqual([
      'confirmed',
      'dismissed',
    ]);
    expect((decisions[0].data as { option?: string }).option).toBe('pro-max');
    expect((decisions[0].data as { action?: string }).action).toBe('select_model');
    expect((decisions[1].data as { option?: string }).option).toBeUndefined();
  } finally {
    await fixture.close();
  }
});
