import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T037：serve 带 --connection/--model 启动（ds 预检单 run.sh 的起法）后，页面跨连接换模型仍然生效。
 * 现场 bug：启动参数压过会话选择，换到 MiniMax 后输入栏和下一轮仍用 deepseek。
 */

const urlOf = (fixture: KernelFixture) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

interface RootFacts {
  /** 根会话里最后一条模型回复的 source.model。 */
  replyModel: string | null;
  /** 最后一个 turn/end 的原因。 */
  lastTurnEnd: string | null;
}

async function rootFacts(fixture: KernelFixture): Promise<RootFacts> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  const rows = text
    .trim()
    .split(String.fromCharCode(10))
    .map((line) => JSON.parse(line)) as {
    type: string;
    data?: {
      message?: { source?: { kind?: string; model?: string } };
      reason?: { kind?: string };
    };
  }[];
  let replyModel: string | null = null;
  let lastTurnEnd: string | null = null;
  for (const row of rows) {
    if (row.type !== 'assistant/message') {
      if (row.type === 'turn/end') lastTurnEnd = row.data?.reason?.kind ?? null;
      continue;
    }
    if (row.data?.message?.source?.kind === 'model') {
      replyModel = row.data.message.source.model ?? null;
    }
  }
  return { replyModel, lastTurnEnd };
}

test('serve 带 --connection/--model 启动：页面跨连接换模型立刻生效，下一轮真的用新模型', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
  const fixture = await startKernelFixture({
    startupSelection: { connection: 'deepseek', model: 'deepseek-flash' },
    vendorModels: ['MiniMax-M3.1-Flash', 'MiniMax-M3'],
  });
  try {
    // 页面里再多接一条 MiniMax 国内站连接（假厂商；显式模型行，换过去不用等列表）。
    const saved = await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'minimax-cn',
      name: 'MiniMax（国内站）',
      model: 'MiniMax-M3.1-Flash',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'sk-e2e-t037',
      preset: 'minimax-cn',
      models: [{ id: 'MiniMax-M3.1-Flash' }],
    });
    expect(saved.ok()).toBeTruthy();
    // 启动参数先把初始选择落成 deepseek。
    const initial = await apiPost(request, fixture, { method: 'status' });
    expect(initial.ok()).toBeTruthy();
    expect((await initial.json()).stats?.model).toBe('deepseek-flash');

    await page.goto(urlOf(fixture));
    await page.locator('[data-kind="model-picker"]').waitFor({ state: 'visible', timeout: 20_000 });
    // 从 DeepSeek 换到 MiniMax 国内站：输入栏按钮立刻显示新模型（不等下一轮）。
    await page.locator('[data-kind="model-picker"]').click();
    const item = page.getByRole('menuitem', { name: /MiniMax-M3\.1-Flash/ });
    await expect(item).toBeVisible({ timeout: 20_000 });
    await item.click();
    await expect(page.locator('[data-kind="model-picker"]')).toContainText(
      'MiniMax-M3.1-Flash',
      { timeout: 20_000 },
    );

    // 下一轮真的用新模型发：对方收到的请求模型、日志里的回复来源、status、turn/end 都对。
    await page.locator('[data-kind="composer-input"]').fill('你好');
    await page.locator('[data-kind="send"]').click();
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toBeVisible({
      timeout: 30_000,
    });
    expect(await fixture.lastModel()).toBe('MiniMax-M3.1-Flash');
    const status = await apiPost(request, fixture, { method: 'status' });
    expect(status.ok()).toBeTruthy();
    const body = (await status.json()) as {
      stats?: { connection?: string; model?: string };
    };
    expect(body.stats?.connection).toBe('minimax-cn');
    expect(body.stats?.model).toBe('MiniMax-M3.1-Flash');
    const facts = await rootFacts(fixture);
    expect(facts.replyModel).toBe('MiniMax-M3.1-Flash');
    expect(facts.lastTurnEnd).toBe('completed');
  } finally {
    await fixture.close();
  }
});
