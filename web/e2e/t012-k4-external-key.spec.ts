import { test, expect } from '@playwright/test';
import { readdir, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { apiPost, startKernelFixture } from './service-fixture';

const LATE_KEY = 'sk-K4-EXTERNAL-KEY-7';

async function monitorText(dataDir: string): Promise<string> {
  const dir = path.join(dataDir, 'monitor');
  const files = (await readdir(dir).catch(() => [])).filter((file) => file.endsWith('.jsonl')).sort();
  const parts: string[] = [];
  for (const file of files) parts.push(await readFile(path.join(dir, file), 'utf8'));
  return parts.join('\n');
}

/// K4（T012.4）：服务运行中从外部 keyring set 换/加 key，签发时刷新脱敏列表，
/// 新 key 不能原样进监控。
test('K4 外部 keyring set 后新 key 也被脱敏', async ({ page, request }) => {
  const fixture = await startKernelFixture({ toolTurn: true });
  try {
    const repoRoot = path.resolve(process.cwd(), '..');
    const bin = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');
    const identity = path.join(fixture.dataDir, 'identity.txt');
    const seeded = spawnSync(
      bin,
      ['keyring', '--data-dir', fixture.dataDir, '--identity', identity, 'set',
        '--id', 'late', '--name', 'Late', '--model', 'deepseek-flash', '--endpoint', 'http://127.0.0.1:1'],
      { input: LATE_KEY + String.fromCharCode(10), encoding: 'utf8' },
    );
    expect(seeded.status, seeded.stderr).toBe(0);

    (await import("node:fs/promises")).writeFile(path.join(fixture.dataDir, "hello.txt"), "工具读到的内容：" + LATE_KEY + "\n");
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });
    await page.getByPlaceholder('和 Kind 说…').fill('读一下 hello.txt');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect
      .poll(
        async () => {
          const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
          const body = (await response.json()) as { events: { type: string }[] };
          return body.events.filter((event) => event.type === 'turn/end').length;
        },
        { timeout: 60_000 },
      )
      .toBeGreaterThanOrEqual(1);

    await expect
      .poll(async () => (await monitorText(fixture.dataDir)).length, { timeout: 20_000 })
      .toBeGreaterThan(0);
    await expect
      .poll(async () => (await monitorText(fixture.dataDir)).includes(LATE_KEY), { timeout: 20_000 })
      .toBe(false);
    expect(await monitorText(fixture.dataDir)).toContain('[已脱敏]');
  } finally {
    await fixture.close();
  }
});
