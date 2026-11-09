import { spawnSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { apiPost, startRealKernelFixture, type KernelFixture } from './service-fixture';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const EVIDENCE = process.env.AIOS_DOD_EVIDENCE ?? path.join(tmpdir(), 'aios-t009-dod-evidence');
const BIN = process.env.AIOS_BIN ?? path.join(repoRoot, 'target/debug/aios-dev');

async function shot(page: Page, name: string) {
  await mkdir(EVIDENCE, { recursive: true });
  await page.screenshot({ path: path.join(EVIDENCE, name) });
}

async function call(request: APIRequestContext, fixture: KernelFixture, body: Record<string, unknown>) {
  const response = await apiPost(request, fixture, body);
  expect(response.ok()).toBeTruthy();
  return response.json();
}

async function eventsOf(request: APIRequestContext, fixture: KernelFixture) {
  const body = await call(request, fixture, { method: 'follow', lastSeq: 0 });
  return body.events as { seq: number; type: string }[];
}

async function waitTurns(request: APIRequestContext, fixture: KernelFixture, count: number) {
  await expect
    .poll(
      async () => (await eventsOf(request, fixture)).filter((event) => event.type === 'turn/end').length,
      { timeout: 180_000 },
    )
    .toBeGreaterThanOrEqual(count);
}

async function openKind(page: Page, fixture: KernelFixture) {
  await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
  await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 60_000 });
}

test.skip(!process.env.DEEPSEEK_API_KEY, 'T009 DoD：需要 DEEPSEEK_API_KEY（运行时注入）');

/// DoD3：真 key 对/错——设置页验证 + 对话；错 key 时中文原因 + 去设置。
test('DoD3 真 key 对和错：验证、对话、中文原因', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    // 对 key：设置页验证可用。
    await openKind(page, fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings');
    await expect(page.getByRole('button', { name: '验证', exact: true })).toBeVisible({ timeout: 30_000 });
    await page.getByRole('button', { name: '验证', exact: true }).click();
    await expect(page.getByText('连接可用', { exact: false })).toBeVisible({ timeout: 60_000 });
    await shot(page, 't009-dod3-verify-ok.png');

    // 对 key：发一条，拿到回答。
    await openKind(page, fixture);
    await page.getByPlaceholder('和 Kind 说…').fill('用一句话回答：今天适合写代码吗？');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await waitTurns(request, fixture, 1);
    await expect(page.getByText('今天', { exact: false }).first()).toBeVisible({ timeout: 60_000 });
    await shot(page, 't009-dod3-chat-ok.png');

    // 错 key：子进程用 stdin 换 key（key 不进 argv/文件）。
    const wrong = 'sk-wrong-t009-do-not-use-' + String(Date.now());
    const identity = path.join(fixture.dataDir, 'identity.txt');
    const seeded = spawnSync(
      BIN,
      ['keyring', '--data-dir', fixture.dataDir, '--identity', identity, 'set', '--id', 'deepseek', '--name', 'DeepSeek', '--model', 'deepseek-flash', '--endpoint', 'https://api.deepseek.com/anthropic'],
      { input: wrong + '\n', encoding: 'utf8' },
    );
    expect(seeded.status, String(seeded.stderr)).toBe(0);

    // 错 key：设置页验证 → 中文「被拒」原因。
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings');
    await page.getByRole('button', { name: '验证', exact: true }).click();
    await expect(page.getByText('拒绝了这把密钥', { exact: false })).toBeVisible({ timeout: 60_000 });
    await shot(page, 't009-dod3-verify-bad.png');

    // 错 key：发一条 → 失败块中文原因 + 去设置。
    await openKind(page, fixture);
    await page.getByPlaceholder('和 Kind 说…').fill('这一条应该被拒绝。');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="failure"]')).toBeVisible({ timeout: 60_000 });
    await expect(page.locator('[data-kind="failure"]')).toContainText('拒绝了这把密钥');
    await expect(page.getByRole('button', { name: /去设置/ })).toBeVisible();
    await shot(page, 't009-dod3-chat-bad.png');
  } finally {
    await fixture.close();
  }
});

/// DoD4：长对话 ≥3 页，往上翻到头，seq 无重复、无缺漏。
test('DoD4 长对话翻页：seq 无重复无缺漏', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    const total = 26;
    for (let index = 1; index <= total; index += 1) {
      await call(request, fixture, {
        method: 'send',
        requestId: 't009-dod4-' + String(index),
        content: '回复数字 ' + String(index),
      });
      await waitTurns(request, fixture, index);
    }
    const all = await eventsOf(request, fixture);
    const maxSeq = all.reduce((max, event) => Math.max(max, event.seq), 0);

    // 服务端分页：从末尾往前翻，每页 6 条消息，直到没有更多。
    const collected: number[] = [];
    let before: number | undefined;
    let pages = 0;
    for (;;) {
      const body = await call(request, fixture, {
        method: 'events',
        ...(before === undefined ? {} : { beforeSeq: before }),
        maxMessages: 6,
        minMessages: 1,
        minTurns: 1,
      });
      const events = body.events as { seq: number }[];
      if (events.length === 0) break;
      collected.push(...events.map((event) => event.seq));
      pages += 1;
      before = events[0].seq;
      if (!body.hasMore || pages > 60) break;
    }
    const sorted = [...collected].sort((left, right) => left - right);
    const unique = new Set(sorted);
    expect(unique.size, 'seq 无重复').toBe(sorted.length);
    expect(sorted[0], '从 1 开始').toBe(1);
    expect(sorted[sorted.length - 1], '覆盖到日志末尾').toBe(maxSeq);
    expect(sorted.length, 'seq 无缺漏').toBe(maxSeq);
    expect(pages).toBeGreaterThanOrEqual(3);

    // 页面：末页只显示最近几轮；往上滚到历史开头。
    await openKind(page, fixture);
    await expect(page.getByText('回复数字 ' + String(total), { exact: false })).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText(/^回复数字 1$/)).toHaveCount(0);
    await page.locator('[data-pane="conversation"]').hover();
    for (let index = 0; index < 12; index += 1) {
      await page.mouse.wheel(0, -3000);
      await page.waitForTimeout(150);
      if (await page.getByText(/^回复数字 1$/).count()) break;
    }
    await expect(page.getByText(/^回复数字 1$/)).toHaveCount(1, { timeout: 60_000 });
    await shot(page, 't009-dod4-history.png');

    await mkdir(EVIDENCE, { recursive: true });
    await writeFile(
      path.join(EVIDENCE, 't009-dod4-pagination.json'),
      JSON.stringify({ total_turns: total, max_seq: maxSeq, pages, collected: sorted.length, unique: unique.size }, null, 2) + '\n',
    );
  } finally {
    await fixture.close();
  }
});
