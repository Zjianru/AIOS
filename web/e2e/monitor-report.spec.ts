import { test, expect } from '@playwright/test';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

async function monitorRecords(dataDir: string): Promise<{ attributes?: Record<string, unknown> }[]> {
  const dir = path.join(dataDir, 'monitor');
  let files: string[];
  try {
    files = (await readdir(dir)).filter((file) => file.endsWith('.jsonl')).sort();
  } catch {
    return [];
  }
  const records: { attributes?: Record<string, unknown> }[] = [];
  for (const file of files) {
    const text = await readFile(path.join(dir, file), 'utf8');
    for (const line of text.split('\n')) {
      if (!line.trim()) continue;
      records.push(JSON.parse(line) as { attributes?: Record<string, unknown> });
    }
  }
  return records;
}

function countOperation(records: { attributes?: Record<string, unknown> }[], op: string): number {
  return records.filter((record) => record.attributes?.['telemetry.op'] === op).length;
}

/// T011.6 清单 10：页面没接住的异常、事件流重连，经 /api monitor-report 进监控；内容限长。
test('页面异常与重连进监控', async ({ page }) => {
  const fixture = await startKernelFixture({ proxy: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });

    // 长消息：服务端限长 512 字符。
    await page.evaluate(() => {
      setTimeout(() => {
        throw new Error('E2E-' + 'x'.repeat(600));
      }, 0);
    });
    await expect
      .poll(
        async () =>
          (await monitorRecords(fixture.dataDir)).filter(
            (record) => record.attributes?.['telemetry.op'] === 'page-error',
          ).length,
        { timeout: 15_000 },
      )
      .toBeGreaterThan(0);

    const errors = (await monitorRecords(fixture.dataDir)).filter(
      (record) => record.attributes?.['telemetry.op'] === 'page-error',
    ) as { attributes: { message?: string } }[];
    expect(String(errors[0].attributes.message ?? '')).toContain('E2E-');
    expect(String(errors[0].attributes.message ?? '').length).toBeLessThanOrEqual(512);

    // 断事件流再恢复：重连成功时报 page-reconnect。
    // 先确认事件流真的连上了（stream-open 已进监控），否则可能在 WS 还没连上时断/恢复，
    // 之后没有可观察的重连可等。
    await expect
      .poll(async () => countOperation(await monitorRecords(fixture.dataDir), 'stream-open'), {
        timeout: 15_000,
      })
      .toBeGreaterThan(0);
    await fixture.dropProxy?.();
    // 页面先进入「正在重连」再恢复代理：可观察状态代替固定 500ms sleep。
    await expect(page.locator('[data-kind="offline"]')).toBeVisible({ timeout: 15_000 });
    await fixture.restoreProxy?.();
    await expect
      .poll(
        async () =>
          (await monitorRecords(fixture.dataDir)).filter(
            (record) => record.attributes?.['telemetry.op'] === 'page-reconnect',
          ).length,
        { timeout: 30_000 },
      )
      .toBeGreaterThan(0);
    // 重连成功后横幅消失：事件流恢复也是可观察的。
    await expect(page.locator('[data-kind="offline"]')).toHaveCount(0, { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});
