import { test, expect, type APIRequestContext } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(tmpdir(), 'aios-t010-replay-evidence');

async function eventsOf(request: APIRequestContext, fixture: KernelFixture) {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  expect(response.ok()).toBeTruthy();
  return (await response.json()).events as { seq: number; type: string; data: Record<string, unknown> }[];
}

async function waitTurns(request: APIRequestContext, fixture: KernelFixture, count: number) {
  await expect
    .poll(async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'turn/end').length, {
      timeout: 120_000,
    })
    .toBeGreaterThanOrEqual(count);
}

async function send(request: APIRequestContext, fixture: KernelFixture, content: string, id: string) {
  const response = await apiPost(request, fixture, { method: 'send', requestId: id, content });
  expect(response.ok()).toBeTruthy();
}

/// T010.5 清单 8：回放真 key 录的夹具——悬停看到模型名和缓存命中、展开看到「想了 N 秒」、分隔线有「占用 a% → b%」。
test('回放真 key 夹具：缓存命中、想了 N 秒、压缩占用', async ({ page, request }) => {
  const fixture = await startKernelFixture({ realTurn: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 60_000 });
    await send(request, fixture, '请先用 read 工具读一下 hello.txt，再告诉我文件里写了什么。', 'replay-real-1');
    await waitTurns(request, fixture, 1);
    await send(request, fixture, '再简短说一次刚才的文件内容。', 'replay-real-2');
    await waitTurns(request, fixture, 2);

    // T013.5：悬停最后一轮整轮行：模型名 + 缓存命中（真 key 录的 usage 有 cacheRead）。
    const lastProcess = page.locator('[data-kind="process"]').last();
    await lastProcess.hover();
    const usageRow = lastProcess.locator('[data-kind="turn-usage"]');
    await expect(usageRow).toContainText(/DeepSeek-V4\.1-Flash/, { timeout: 20_000 });
    await expect(usageRow).toContainText(/输入\s*\d+/);
    await expect(usageRow).toContainText(/缓存命中\s*\d+%/);

    // 展开第一轮：思考帧 → 想了 N 秒。
    await page.locator('[data-kind="process"]').first().locator('button').first().click();
    await expect(page.getByText(/想了 \d+ 秒/)).toBeVisible({ timeout: 20_000 });

    // 手动压缩 + 再发一条：分隔线出现占用 a% → b%。
    const compact = await apiPost(request, fixture, { method: 'compact', commandId: 'replay-real-compact' });
    expect(compact.ok()).toBeTruthy();
    await expect
      .poll(async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'compaction/end').length, {
        timeout: 120_000,
      })
      .toBeGreaterThanOrEqual(1);
    await send(request, fixture, '压缩之后再简短说一句。', 'replay-real-3');
    await waitTurns(request, fixture, 3);
    const divider = page.locator('[data-kind="compaction"]').last();
    await expect(divider).toContainText(/占用 \d+% → \d+%/, { timeout: 30_000 });

    await mkdir(EVIDENCE, { recursive: true });
    await page.screenshot({ path: path.join(EVIDENCE, 't010-e2e-replay.png') });
  } finally {
    await fixture.close();
  }
});
