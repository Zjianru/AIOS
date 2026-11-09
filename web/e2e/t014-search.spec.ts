import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const EXA_KEY = 'exa-search-e2e-key-0123456789';
const SEARCH_URL = 'https://example.com/aios-search';
const ANSWER = '（T014.7 e2e 回放）搜到了：';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

/** 数据目录里不能出现 key 正文（日志、监控、Keyring 密文一起查）。 */
async function expectNoKeyInDataDir(dataDir: string, secret: string): Promise<void> {
  const walk = async (dir: string): Promise<void> => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        await walk(full);
        continue;
      }
      const bytes = await readFile(full);
      expect(bytes.includes(Buffer.from(secret)), full + ' 里出现了 key 正文').toBe(false);
    }
  };
  await walk(dataDir);
}

/** 根会话日志里 web_search 的 tool/result 正文（证明来源 URL 真的来自搜索结果）。 */
async function searchToolResult(fixture: KernelFixture): Promise<string> {
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
    if (row.type !== 'tool/result' || row.data?.message?.toolCallId !== 'call_web_search') continue;
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

/// T014.7 / 清单 13 / 故事 8：加载 web 后搜一次——工作过程里有「搜网页」一行，
/// 回答引用结果里的 URL；授权头来自设置页存的 Keyring key，key 正文不落数据目录。
test('联网搜索：工作过程有「搜网页」，回答引用结果里的 URL', async ({ page }) => {
  const fixture = await startKernelFixture({ t014Search: true, fakeExa: true });
  try {
    await saveExaKey(page, fixture);

    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('帮我搜一下 AIOS 的原生工具');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toContainText(ANSWER, { timeout: 30_000 });
    await expect(answer.getByRole('link', { name: 'AIOS 原生工具' })).toHaveAttribute(
      'href',
      SEARCH_URL,
    );
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, {
      timeout: 60_000,
    });

    const process = page.locator('[data-kind="process"]').last();
    await expect(process.locator('button').first()).toContainText(/已完成，用时/);
    await process.locator('button').first().click();
    await process.locator('[data-kind="process-group"] button').first().click();
    const rows = process.locator('[data-kind="tool-row"]');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toHaveAttribute('data-tool', 'skill');
    await expect(rows.nth(1)).toHaveAttribute('data-tool', 'web_search');
    await expect(rows.nth(1)).toContainText('搜网页');
    await expect(rows.nth(1)).toContainText('AIOS 原生工具');

    // 搜索的 tool/result 里有假 Exa 返回的来源 URL；key 不进结果。
    const toolResult = await searchToolResult(fixture);
    expect(toolResult).toContain('Sources:');
    expect(toolResult).toContain(SEARCH_URL);
    expect(toolResult).not.toContain(EXA_KEY);

    // 搜索请求的 authorization 来自设置页存进 Keyring 的那把 key。
    const authorizations = fixture.exa?.requests() ?? [];
    expect(authorizations).toContain('Bearer ' + EXA_KEY);

    await expectNoKeyInDataDir(fixture.dataDir, EXA_KEY);
  } finally {
    await fixture.close();
  }
});
