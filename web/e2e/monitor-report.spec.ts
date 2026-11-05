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
    await fixture.dropProxy?.();
    await new Promise((resolve) => setTimeout(resolve, 500));
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
  } finally {
    await fixture.close();
  }
});
