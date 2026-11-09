import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const EXA_KEY = 'exa-fetch-e2e-key-0123456789';
const FETCH_URL = 'http://203.0.113.10/aios-fetch';
const ANSWER = '（T014.8 e2e 回放）';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** 根会话日志里某次 web_fetch 的 tool/result 正文。 */
async function fetchToolResult(fixture: KernelFixture, callId: string): Promise<string> {
  const root = JSON.parse(
    await readFile(path.join(fixture.dataDir, 'kind-root.json'), 'utf8'),
  ) as { sessionId: string };
  const text = await readFile(
    path.join(fixture.dataDir, 'sessions', root.sessionId + '.jsonl'),
    'utf8',
  );
  for (const line of text.trim().split(String.fromCharCode(10))) {
    const row = JSON.parse(line) as {
      type?: string;
      data?: { message?: { toolCallId?: string; content?: { text?: string }[] } };
    };
    if (row.type !== 'tool/result' || row.data?.message?.toolCallId !== callId) continue;
    return (row.data.message.content ?? []).map((block) => block.text ?? '').join('');
  }
  return '';
}

async function saveExaKey(page: Page, fixture: KernelFixture): Promise<void> {
  await page.goto(urlOf(fixture));
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const section = page.locator('[data-settings="web-search"]');
  await expect(section).toBeVisible({ timeout: 20_000 });
  await section.locator('[data-settings="exa-key-input"]').fill(EXA_KEY);
  await section.getByRole('button', { name: '保存', exact: true }).click();
  await expect(section.locator('[data-settings="web-search-status"]')).toContainText('能用（', {
    timeout: 20_000,
  });
}

/// T014.8 / 清单 12、14 / 故事 10：加载 web 后读一个网页，正文进工作过程；
/// 读本机地址的那一行是拒绝。公网地址经本地假代理回正文，本机地址在发请求前就被拦下。
test('读网页：公网正文进工作过程，本机地址那行是拒绝', async ({ page }) => {
  const fixture = await startKernelFixture({ t014Fetch: true, fakeExa: true });
  try {
    await saveExaKey(page, fixture);

    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('读一下这个链接，再试试本机地址');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(ANSWER, {
      timeout: 30_000,
    });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, {
      timeout: 60_000,
    });

    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/);
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    const rows = process.locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toHaveAttribute('data-tool', 'skill');
    await expect(rows.nth(1)).toHaveAttribute('data-tool', 'web_fetch');
    await expect(rows.nth(1)).toContainText('读网页');
    await expect(rows.nth(1)).toContainText(FETCH_URL);
    await expect(rows.nth(2)).toHaveAttribute('data-tool', 'web_fetch');
    await expect(rows.nth(2)).toHaveAttribute('data-state', 'error');
    await expect(rows.nth(2)).toContainText('本机地址');

    // 公网的 tool/result 有正文，没有脚本；本机的那条是拒绝。
    const publicResult = await fetchToolResult(fixture, 'call_web_fetch_public');
    expect(publicResult).toContain('Fetched ' + FETCH_URL + ' (HTTP 200)');
    expect(publicResult).toContain('读网页正文');
    expect(publicResult).toContain('T014.8 e2e 正文');
    expect(publicResult).not.toContain('脚本不该出现');
    expect(publicResult).not.toContain(EXA_KEY);

    const localResult = await fetchToolResult(fixture, 'call_web_fetch_local');
    expect(localResult).toContain('本机地址');
    expect(localResult).toContain('不能读');

    // 假代理只收到公网那一条；本机地址没发出去。
    const requests = fixture.fetch?.requests() ?? [];
    expect(requests).toHaveLength(1);
    expect(requests[0]).toContain(FETCH_URL);
    expect(requests.join(' ')).not.toContain('127.0.0.1:1');
  } finally {
    await fixture.close();
  }
});
