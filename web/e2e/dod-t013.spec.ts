import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { startT013KernelFixture } from './service-fixture';

const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? '/tmp/aios-t013-dod/evidence';
const ROOT = process.env.AIOS_T013_DOD_ROOT ?? '/tmp/aios-t013-dod';
const jqLog: string[] = [];

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（T013.9 真 key DoD）');
test.setTimeout(300_000);

function sessionLog(dataDir: string): string {
  const dir = path.join(dataDir, 'sessions');
  const file = readdirSync(dir).filter((name) => name.endsWith('.jsonl')).sort().pop();
  if (!file) throw new Error('没有会话日志');
  return path.join(dir, file);
}

/** DoD2：每件事附 jq 命令和输出（只提取要求的三样）。 */
function jqExamples(dataDir: string, tag: string): void {
  const log = sessionLog(dataDir);
  const run = (args: string[], label: string) => {
    const out = execFileSync('jq', args, { encoding: 'utf8' }).trim();
    jqLog.push('$ jq ' + args.join(' ') + '  # ' + label + String.fromCharCode(10) + out);
  };
  run(['-c', 'select(.type=="user/message") | .data.message.content', log], tag + ' user/message 块');
  run(['-c', 'select(.type=="tool/call") | {name: .data.name, arguments: .data.arguments}', log], tag + ' tool/call（路径在 arguments）');
  run(['-c', 'select(.type=="tool/result") | .data.meta.diffs', log], tag + ' tool/result.meta');
}

function flushJq(): void {
  mkdirSync(EVIDENCE, { recursive: true });
  writeFileSync(path.join(EVIDENCE, 'dod2-jq.txt'), jqLog.join(String.fromCharCode(10) + String.fromCharCode(10)));
}

test.afterAll(() => {
  flushJq();
});

async function eventsOf(request: import('@playwright/test').APIRequestContext, fixture: { baseUrl: string; token: string }) {
  const response = await request.post(fixture.baseUrl + '/api', {
    headers: { authorization: 'Bearer ' + fixture.token, 'content-type': 'application/json' },
    data: { method: 'events', maxMessages: 200 },
  });
  const json = await response.json();
  return (json.events ?? []) as { type: string; data?: Record<string, any> }[];
}

async function waitTurnEnd(request: import('@playwright/test').APIRequestContext, fixture: { baseUrl: string; token: string }, count = 1) {
  await expect
    .poll(async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'turn/end').length, { timeout: 180_000 })
    .toBeGreaterThanOrEqual(count);
}

const send = async (page: import('@playwright/test').Page, text: string) => {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.locator('[data-kind="send"]').click();
};

/// DoD2 ①：@ 一个文件问内容——日志里在回答之前有一次 read 那个路径。
test('DoD2①：@ 文件问内容', async ({ page, request }) => {
  const fixture = await startT013KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const box = page.locator('[data-kind="composer-input"]');
    await box.fill('@notes.md');
    await page.locator('[data-kind="at-menu"]').getByRole('button', { name: /notes\.md/ }).first().click();
    await box.fill('@notes.md 这个文件第一行写了什么？原样告诉我。');
    await page.locator('[data-kind="send"]').click();
    await expect(page.getByText('DOD13-NOTES', { exact: false }).last()).toBeVisible({ timeout: 180_000 });
    await waitTurnEnd(request, fixture, 1);
    const events = await eventsOf(request, fixture);
    const calls = events.filter((event) => event.type === 'tool/call').map((event) => JSON.stringify(event.data?.arguments ?? ''));
    expect(calls.some((args) => args.includes('notes.md'))).toBe(true);
    jqExamples(fixture.dataDir, '① @ 文件');
  } finally {
    await fixture.close();
  }
});

/// DoD2 ②：拖进一个文件问内容——日志里有 read 副本路径。
test('DoD2②：拖文件问内容（副本路径）', async ({ page, request }) => {
  const fixture = await startT013KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await page.locator('[data-kind="composer"]').evaluate((element) => {
      const data = new DataTransfer();
      data.items.add(new File(['DOD13-DROP：拖进来的文件内容' + String.fromCharCode(10)], 'dropped.txt', { type: 'text/plain' }));
      element.dispatchEvent(new DragEvent('drop', { dataTransfer: data, bubbles: true, cancelable: true }));
    });
    await expect(page.locator('[data-kind="file-card"]')).toHaveCount(1, { timeout: 20_000 });
    await send(page, '拖进来的这个文件里写了什么？原样告诉我。');
    await expect(page.getByText('DOD13-DROP', { exact: false }).last()).toBeVisible({ timeout: 180_000 });
    await waitTurnEnd(request, fixture, 1);
    const events = await eventsOf(request, fixture);
    const calls = events.filter((event) => event.type === 'tool/call').map((event) => JSON.stringify(event.data?.arguments ?? ''));
    expect(calls.some((args) => args.includes('attachments/v1/files'))).toBe(true);
    jqExamples(fixture.dataDir, '② 拖文件');
  } finally {
    await fixture.close();
  }
});

/// DoD2 ③：粘贴 >1000 字符问内容——答对。
test('DoD2③：大段粘贴问内容', async ({ page, request }) => {
  const fixture = await startT013KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const marker = 'DOD13-PASTE-MARKER';
    const content = marker + String.fromCharCode(10) + 'x'.repeat(1200);
    await page.locator('[data-kind="composer-input"]').evaluate((element, value) => {
      const data = new DataTransfer();
      data.setData('text/plain', value);
      element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }));
    }, content);
    await expect(page.locator('[data-kind="paste-card"]')).toHaveCount(1);
    await send(page, '我粘贴的这段文字里第一行有什么标记？原样告诉我。');
    await expect(page.getByText(marker, { exact: false }).last()).toBeVisible({ timeout: 180_000 });
    await waitTurnEnd(request, fixture, 1);
    jqExamples(fixture.dataDir, '③ 大段粘贴');
  } finally {
    await fixture.close();
  }
});

/// DoD2 ④：让它改文件——diff 卡片出来，刷新后还在。
test('DoD2④：改文件（diff 卡片，刷新后还在）', async ({ page, request }) => {
  const fixture = await startT013KernelFixture();
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await send(page, '把 notes.md 里的 DOD13-NOTES 改成 DOD13-EDITED。只改这一处，别做别的。');
    await waitTurnEnd(request, fixture, 1);
    await expect
      .poll(async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'tool/result' && JSON.stringify(event.data?.meta ?? '').includes('diffs')).length, {
        timeout: 180_000,
      })
      .toBeGreaterThanOrEqual(1);
    // diff 卡片（工作过程展开后的 DiffCard）。
    const process = page.locator('[data-kind="process"]').last();
    await process.locator('button').first().click();
    const group = process.locator('[data-kind="process-group"] button').first();
    if (await group.count()) await group.click();
    await expect(process.getByText('DOD13-EDITED', { exact: false }).first()).toBeVisible({ timeout: 60_000 });
    await page.reload();
    await expect(page.locator('[data-kind="process"]').last()).toBeVisible({ timeout: 60_000 });
    const process2 = page.locator('[data-kind="process"]').last();
    await process2.locator('button').first().click();
    const group2 = process2.locator('[data-kind="process-group"] button').first();
    if (await group2.count()) await group2.click();
    await expect(process2.getByText('DOD13-EDITED', { exact: false }).first()).toBeVisible({ timeout: 60_000 });
    jqExamples(fixture.dataDir, '④ 改文件');
  } finally {
    await fixture.close();
  }
});
