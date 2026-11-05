import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { mkdir, readdir, readFile, copyFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startRealKernelFixture, type KernelFixture } from './service-fixture';

const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? '/Users/codez/develop/aios-work/T010/evidence/dod';
const DOD_ROOT = process.env.AIOS_DOD_ROOT ?? '/tmp/aios-t010-dod';

async function shot(page: Page, name: string) {
  await mkdir(EVIDENCE, { recursive: true });
  await page.screenshot({ path: path.join(EVIDENCE, name), fullPage: false });
}

async function eventsOf(request: APIRequestContext, fixture: KernelFixture) {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  expect(response.ok()).toBeTruthy();
  return (await response.json()).events as { seq: number; type: string; data: Record<string, unknown> }[];
}

async function waitTurns(request: APIRequestContext, fixture: KernelFixture, count: number) {
  await expect
    .poll(async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'turn/end').length, {
      timeout: 180_000,
    })
    .toBeGreaterThanOrEqual(count);
}

async function send(request: APIRequestContext, fixture: KernelFixture, content: string, id: string) {
  const response = await apiPost(request, fixture, { method: 'send', requestId: id, content });
  expect(response.ok()).toBeTruthy();
}

interface UsageLike {
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
}

function contextOf(usage: UsageLike | undefined): number {
  return (usage?.inputTokens ?? 0) + (usage?.cacheReadTokens ?? 0) + (usage?.cacheWriteTokens ?? 0);
}

test.skip(!process.env.DEEPSEEK_API_KEY, '真 key DoD：需要 DEEPSEEK_API_KEY（运行时注入）');

test('DoD2/3/4 真 key：缓存、压缩占用、用量、夹具', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 60_000 });

    await send(request, fixture, '请先用 read 工具读一下 hello.txt，再告诉我文件里写了什么。', 't010-dod-1');
    await waitTurns(request, fixture, 1);
    await send(request, fixture, '再简短说一次刚才的文件内容。', 't010-dod-2');
    await waitTurns(request, fixture, 2);

    let events = await eventsOf(request, fixture);
    const messages = events.filter((event) => event.type === 'assistant/message');
    const toolCalls = events.filter((event) => event.type === 'tool/call');
    expect(toolCalls.length, 'DoD2 需要一次多步工具调用').toBeGreaterThanOrEqual(1);

    // DoD2：除第一条外，每条 assistant/message 的 cacheReadTokens > 0。
    const usages = messages.map((event) => (event.data.usage ?? undefined) as UsageLike | undefined);
    expect(usages.length).toBeGreaterThanOrEqual(2);
    const cacheLines: string[] = [];
    usages.forEach((usage, index) => {
      const cacheRead = usage?.cacheReadTokens ?? 0;
      cacheLines.push(
        'assistant/message #' +
          String(index + 1) +
          ' input=' +
          String(usage?.inputTokens ?? 'missing') +
          ' output=' +
          String(usage?.outputTokens ?? 'missing') +
          ' cacheRead=' +
          String(usage?.cacheReadTokens ?? 'missing') +
          ' cacheWrite=' +
          String(usage?.cacheWriteTokens ?? 'missing'),
      );
      if (index > 0) expect(cacheRead, '除第一条外 cacheReadTokens 必须 > 0：' + cacheLines.join(' | ')).toBeGreaterThan(0);
    });
    await mkdir(EVIDENCE, { recursive: true });
    await expect(page.getByText('（T007 e2e 假回放）', { exact: false })).toHaveCount(0);

    // DoD4：悬停任意一轮，模型名与设置显示名一致、缓存命中与日志手算一致。
    const window = 1_000_000; // 模型设置 catalog：deepseek-flash 的 contextWindow（keyring.ts）
    const second = usages[usages.length - 1] ?? {};
    const shownInput = contextOf(second);
    const cacheHit = Math.round(((second.cacheReadTokens ?? 0) / (shownInput || 1)) * 100);
    const row = page.getByText('工作过程', { exact: false }).last();
    await row.hover();
    await expect(page.getByText('DeepSeek Flash · 输入 ' + String(shownInput), { exact: false }).last()).toBeVisible({
      timeout: 20_000,
    });
    await expect(page.getByText('缓存命中 ' + String(cacheHit) + '%', { exact: false }).last()).toBeVisible();
    await shot(page, 't010-dod4-usage.png');

    // DoD3：手动压缩一次，再发一条；分隔线的占用 a% → b% 与日志手算一致。
    const compactResponse = await apiPost(request, fixture, { method: 'compact', commandId: 't010-dod-compact' });
    expect(compactResponse.ok()).toBeTruthy();
    await expect
      .poll(async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'compaction/end').length, {
        timeout: 180_000,
      })
      .toBeGreaterThanOrEqual(1);
    await send(request, fixture, '压缩之后再简短说一句。', 't010-dod-3');
    await waitTurns(request, fixture, 3);

    events = await eventsOf(request, fixture);
    const summary = events.filter((event) => event.type === 'compaction/summary').at(-1);
    expect(summary, '压缩摘要事件').toBeTruthy();
    const summaryUsage = (summary?.data.usage ?? undefined) as UsageLike | undefined;
    const before = Math.round((contextOf(summaryUsage) / window) * 100);
    const afterUsage = events
      .filter((event) => event.type === 'assistant/message' && event.seq > (summary?.seq ?? 0))
      .at(0)?.data.usage as UsageLike | undefined;
    const after = Math.round((contextOf(afterUsage) / window) * 100);
    const divider = page.locator('[data-kind="compaction"]').last();
    await expect(divider).toContainText('占用 ' + String(before) + '% → ' + String(after) + '%', { timeout: 20_000 });
    await divider.hover();
    await shot(page, 't010-dod3-compaction.png');

    // 夹具：这次真跑的会话日志（DoD5）；先落到 DoD 目录，扫 key 后再复制进仓库。
    const sessions = await readdir(path.join(fixture.dataDir, 'sessions'));
    expect(sessions.filter((file) => file.endsWith('.jsonl')).length).toBe(1);
    await copyFile(
      path.join(fixture.dataDir, 'sessions', sessions.find((file) => file.endsWith('.jsonl')) ?? ''),
      path.join(DOD_ROOT, 't010-real-fixture.jsonl'),
    );
    await mkdir(path.join(DOD_ROOT, 'evidence'), { recursive: true });
    await readFile(path.join(fixture.dataDir, 'sessions', sessions[0]));
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });
    console.log('DoD2 usage lines: ' + JSON.stringify(cacheLines));
    console.log('DoD3 compaction: before=' + String(before) + '% after=' + String(after) + '%');
  } finally {
    await fixture.close();
  }
});
