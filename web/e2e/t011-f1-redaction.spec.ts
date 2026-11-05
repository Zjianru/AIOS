import { test, expect, type APIRequestContext } from '@playwright/test';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const STARTUP_KEY = 'test-key';
const LATE_KEY = 'sk-F1-LATE-KEY-9';

async function call(request: APIRequestContext, fixture: KernelFixture, body: Record<string, unknown>) {
  const response = await apiPost(request, fixture, body);
  expect(response.ok()).toBeTruthy();
  return response.json();
}

async function waitTurns(request: APIRequestContext, fixture: KernelFixture, count: number) {
  await expect
    .poll(
      async () => {
        const body = await call(request, fixture, { method: 'follow', lastSeq: 0 });
        return (body.events as { type: string }[]).filter((event) => event.type === 'turn/end').length;
      },
      { timeout: 60_000 },
    )
    .toBeGreaterThanOrEqual(count);
}

async function monitorText(dataDir: string): Promise<string> {
  const dir = path.join(dataDir, 'monitor');
  const files = (await readdir(dir).catch(() => [])).filter((file) => file.endsWith('.jsonl')).sort();
  const parts: string[] = [];
  for (const file of files) {
    parts.push(await readFile(path.join(dir, file), 'utf8'));
  }
  return parts.join('\n');
}

/// F1：生产装配（真 aios-dev serve）的监控记录必须脱敏——
/// 启动时 Keyring 里的 key、启动后才保存的 key、ops 记录里的 key，都不能出现原文。
test('F1 生产监控 key 脱敏：启动 key / 运行中保存 key / ops', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });

    // 1) 启动时 Keyring 里的 key 被贴进对话 → ledger 必须脱敏。
    await page.getByPlaceholder('和 Kind 说…').fill('我的启动 key 是 ' + STARTUP_KEY + '，别记下来。');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await waitTurns(request, fixture, 1);

    // 2) 启动后才保存新连接（新 key）→ 之后 ledger 里的新 key 也要脱敏。
    await call(request, fixture, {
      method: 'connection-save',
      id: 'late-f1',
      name: 'Late F1',
      model: 'deepseek-flash',
      endpoint: 'http://127.0.0.1:1',
      apiKey: LATE_KEY,
    });
    await page.getByPlaceholder('和 Kind 说…').fill('后存的 key 是 ' + LATE_KEY + '。');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await waitTurns(request, fixture, 2);

    // 3) ops 记录（页面异常上报）里带 key → 也必须脱敏。
    await call(request, fixture, { method: 'monitor-report', kind: 'page-error', message: 'ops leak ' + LATE_KEY });

    await expect
      .poll(async () => (await monitorText(fixture.dataDir)).length, { timeout: 20_000 })
      .toBeGreaterThan(0);
    await expect
      .poll(
        async () => {
          const text = await monitorText(fixture.dataDir);
          return text.includes(STARTUP_KEY) || text.includes(LATE_KEY);
        },
        { timeout: 20_000 },
      )
      .toBe(false);

    const text = await monitorText(fixture.dataDir);
    expect(text).toContain('[已脱敏]');
    expect(text, 'redact-failed 不应出现').not.toContain('redact-failed');
  } finally {
    await fixture.close();
  }
});
