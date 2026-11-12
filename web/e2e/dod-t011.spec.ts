import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { apiPost, startRealKernelFixture, type KernelFixture } from './service-fixture';
import { verifyConnectionViaDialog } from './verify-dialog';

const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(tmpdir(), 'aios-t011-dod-evidence');
const DOD_ROOT = process.env.AIOS_DOD_ROOT ?? '/tmp/aios-t011-dod';

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

async function monitorRecords(dataDir: string) {
  const dir = path.join(dataDir, 'monitor');
  const files = (await readdir(dir)).filter((file) => file.endsWith('.jsonl')).sort();
  const records: Record<string, unknown>[] = [];
  const raw: string[] = [];
  for (const file of files) {
    const text = await readFile(path.join(dir, file), 'utf8');
    raw.push(text);
    for (const line of text.split('\n')) {
      if (line.trim()) records.push(JSON.parse(line) as Record<string, unknown>);
    }
  }
  return { records, text: raw.join('\n') };
}

async function sessionEventCount(dataDir: string): Promise<number> {
  const dir = path.join(dataDir, 'sessions');
  const files = (await readdir(dir)).filter((file) => file.endsWith('.jsonl'));
  let count = 0;
  for (const file of files) {
    const lines = (await readFile(path.join(dir, file), 'utf8')).split('\n').filter((line) => line.trim());
    count += lines.length - 1; // 去掉 header
  }
  return count;
}

async function sessionReasoningTexts(dataDir: string): Promise<string[]> {
  const dir = path.join(dataDir, 'sessions');
  const files = (await readdir(dir)).filter((file) => file.endsWith('.jsonl'));
  const texts: string[] = [];
  for (const file of files) {
    for (const line of (await readFile(path.join(dir, file), 'utf8')).split('\n')) {
      if (!line.trim()) continue;
      const event = JSON.parse(line) as { data?: { stream?: { type?: string; texts?: string[] }[] } };
      for (const record of event.data?.stream ?? []) {
        if (record.type === 'reasoning-chunks') texts.push(...(record.texts ?? []));
      }
    }
  }
  return texts.filter((text) => text.length >= 4);
}

async function shot(page: Page, name: string) {
  await mkdir(EVIDENCE, { recursive: true });
  await page.screenshot({ path: path.join(EVIDENCE, name) });
}

test.skip(!process.env.DEEPSEEK_API_KEY, 'T011 真 key DoD：需要 DEEPSEEK_API_KEY（运行时注入）');

test('DoD2/3/5/6 真 key：监控覆盖全部环节、日志副本、key/思考不落盘', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 60_000 });
    await apiPost(request, fixture, { method: 'send', requestId: 't011-dod-1', content: '请先用 read 工具读一下 hello.txt，再告诉我文件里写了什么。' });
    await waitTurns(request, fixture, 1);
    await apiPost(request, fixture, { method: 'send', requestId: 't011-dod-2', content: '再简短说一次。' });
    await waitTurns(request, fixture, 2);
    expect((await apiPost(request, fixture, { method: 'compact', commandId: 't011-dod-compact' })).ok()).toBeTruthy();
    await expect
      .poll(async () => (await eventsOf(request, fixture)).some((event) => event.type === 'compaction/end'), {
        timeout: 180_000,
      })
      .toBeTruthy();
    // 设置页验证一次连接（手动验证走弹窗，结果只在弹窗里）。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await verifyConnectionViaDialog(page);
    await shot(page, 't011-dod-settings.png');
    // 关掉页面让事件流断开，才能看到 stream-close。
    await page.close();
    await expect
      .poll(
        async () =>
          (await monitorRecords(fixture.dataDir)).records.filter(
            (record) => (record.attributes as Record<string, unknown>)?.['telemetry.op'] === 'stream-close',
          ).length,
        { timeout: 10_000 },
      )
      .toBeGreaterThanOrEqual(1);

    const { records, text } = await monitorRecords(fixture.dataDir);
    const opCount = (op: string) =>
      records.filter((record) => (record.attributes as Record<string, unknown>)?.['telemetry.op'] === op).length;
    const ledger = records.filter((record) => record.channel === 'ledger').length;
    const sessionEvents = await sessionEventCount(fixture.dataDir);
    expect(ledger, 'ledger 条数 = 会话日志事件数').toBe(sessionEvents);
    expect(opCount('process-start')).toBe(1);
    expect(opCount('api-call')).toBeGreaterThanOrEqual(5);
    expect(opCount('keyring')).toBeGreaterThanOrEqual(1);
    expect(opCount('connection')).toBeGreaterThanOrEqual(1);
    expect(opCount('stream-open')).toBeGreaterThanOrEqual(1);
    expect(opCount('stream-close')).toBeGreaterThanOrEqual(1);
    const errors = records.filter((record) => record.severity === 'error');
    expect(errors.length, 'error 条目应为 0：' + JSON.stringify(errors)).toBe(0);

    // key 前后 8 位与思考原文都不在监控里。
    const apiKey = process.env.DEEPSEEK_API_KEY ?? '';
    expect(apiKey.length).toBeGreaterThan(16);
    expect(text.includes(apiKey.slice(0, 8)), 'key 前 8 位 0 命中').toBeFalsy();
    expect(text.includes(apiKey.slice(-8)), 'key 后 8 位 0 命中').toBeFalsy();
    const reasoning = await sessionReasoningTexts(fixture.dataDir);
    for (const sentence of reasoning) {
      expect(text.includes(sentence), '推理原文 0 命中').toBeFalsy();
    }

    await mkdir(EVIDENCE, { recursive: true });
    await writeFile(
      path.join(EVIDENCE, 't011-dod2-counts.json'),
      JSON.stringify(
        {
          ledger,
          sessionEvents,
          ops: Object.fromEntries(
            [
              'process-start',
              'api-call',
              'keyring',
              'connection',
              'stream-open',
              'stream-close',
              'agent-warn',
              'gap',
            ].map((op) => [op, opCount(op)]),
          ),
          severity_error: errors.length,
          key_hits: 0,
          reasoning_hits: 0,
          reasoning_chunks_checked: reasoning.length,
          monitor_bytes: text.length,
        },
        null,
        2,
      ) + '\n',
    );
    await writeFile(path.join(EVIDENCE, 't011-dod3-reasoning-count.txt'), String(reasoning.length) + '\n');
  } finally {
    await fixture.close();
  }
});
