import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const EXA_KEY = 'exa-e2e-key-0123456789';
const EXA_BAD = 'bad-key-e2e-0123456789';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function openSettings(page: Page, fixture: KernelFixture): Promise<void> {
  await page.goto(urlOf(fixture));
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
}

async function sendAndWaitAnswer(page: Page, question: string, answer: string): Promise<void> {
  await page.getByPlaceholder('和 Kind 说…').fill(question);
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(answer, {
    timeout: 30_000,
  });
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, {
    timeout: 60_000,
  });
}

function toolNames(request: Record<string, unknown> | null): string[] {
  const tools = request?.tools as { name?: string }[] | undefined;
  return (tools ?? []).map((tool) => tool.name ?? '');
}

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

/// T014.6：设置页选 Exa、填/换/删 key、检测；有 key 时 skill web 进目录，
/// 加载以后的下一步工具表里有 web_search/web_fetch（参数也在）。
test('联网搜索：选 Exa、填/换/删 key 与检测；有 key 时 skill web 开出网页工具', async ({ page }) => {
  const fixture = await startKernelFixture({ t014Web: true, fakeExa: true });
  try {
    await openSettings(page, fixture);
    const section = page.locator('[data-settings="web-search"]');
    const status = section.locator('[data-settings="web-search-status"]');
    const detail = section.locator('[data-settings="web-search-detail"]');
    await expect(section).toBeVisible({ timeout: 20_000 });
    await expect(section.locator('[data-web-provider]')).toHaveCount(20);
    await expect(status).toContainText('还没有选服务商');

    await section.locator('[data-web-select]').selectOption('exa');
    await section.locator('[data-web-host]').fill(fixture.exa!.baseUrl);
    await section.locator('[data-web-host-save]').click();
    await expect(status).toContainText('这家要 key');

    await section.locator('[data-web-key-input]').fill(EXA_KEY);
    await section.locator('[data-web-key-save]').click();
    await expect(detail).toContainText('存于');
    await section.locator('[data-web-check]').click();
    await expect(detail).toContainText('上次检查', { timeout: 20_000 });

    // 换坏 key：检测写 401 原因，页面不回显 key。
    await section.locator('[data-web-key-input]').fill(EXA_BAD);
    await section.locator('[data-web-key-save]').click();
    await section.locator('[data-web-check]').click();
    await expect(detail).toContainText('401', { timeout: 20_000 });
    await expect(section.locator('[data-web-key-input]')).toHaveValue('');

    await section.locator('[data-web-key-input]').fill(EXA_KEY);
    await section.locator('[data-web-key-save]').click();
    await section.locator('[data-web-check]').click();
    await expect(detail).toContainText('上次检查', { timeout: 20_000 });

    const authorizations = fixture.exa?.requests() ?? [];
    expect(authorizations).toContain('Bearer ' + EXA_KEY);
    expect(authorizations).toContain('Bearer ' + EXA_BAD);

    // 回到 Kind：模型先加载 web，再收尾；下一步请求里出现网页工具和参数。
    await page.goto(urlOf(fixture));
    await sendAndWaitAnswer(page, '帮我搜一下今天的新闻', '（T014.6 e2e 回放）');
    const request = await fixture.lastRequest();
    expect(request, '要拿到模型请求').toBeTruthy();
    const body = JSON.stringify(request);
    expect(body).toContain('- \`web\`:');
    expect(toolNames(request)).toEqual(expect.arrayContaining(['web_search', 'web_fetch']));
    const tools = (request?.tools ?? []) as Record<string, any>[];
    const search = tools.find((tool) => tool.name === 'web_search');
    const fetch = tools.find((tool) => tool.name === 'web_fetch');
    expect(search?.input_schema?.properties?.queries?.maxItems).toBe(4);
    expect(fetch?.input_schema?.required).toEqual(['url']);

    // 删：立即回到「这家要 key」；Keyring 里不再有 key。
    await openSettings(page, fixture);
    const reopened = page.locator('[data-settings="web-search"]');
    await expect(reopened.locator('[data-settings="web-search-detail"]')).toContainText('存于', { timeout: 20_000 });
    await reopened.locator('[data-web-key-delete]').click();
    await expect(reopened.locator('[data-settings="web-search-status"]')).toContainText('这家要 key', { timeout: 20_000 });

    await expectNoKeyInDataDir(fixture.dataDir, EXA_KEY);
    await expectNoKeyInDataDir(fixture.dataDir, EXA_BAD);
  } finally {
    await fixture.close();
  }
});

/// T014.6：没填 key 时服务不读 EXA_API_KEY；目录里没有 web（留一句去哪里填），
/// 工具表里没有网页工具，Kind 的回答也指去设置页。
test('没填 key：web 不进目录、网页工具不进表；EXA_API_KEY 环境变量不算数', async ({ page }) => {
  const fixture = await startKernelFixture({ envExaKey: 'env-should-not-be-read' });
  try {
    await openSettings(page, fixture);
    const section = page.locator('[data-settings="web-search"]');
    await expect(section.locator('[data-settings="web-search-status"]')).toContainText(
      '还没有选服务商',
      { timeout: 20_000 },
    );

    await page.goto(urlOf(fixture));
    await sendAndWaitAnswer(page, '帮我搜一下今天的新闻', '（T007 e2e 假回放）');
    const request = await fixture.lastRequest();
    expect(request, '要拿到模型请求').toBeTruthy();
    const names = toolNames(request);
    expect(names).not.toContain('web_search');
    expect(names).not.toContain('web_fetch');
    const body = JSON.stringify(request);
    expect(body).toContain('(unavailable)');
    expect(body).toContain('设置 › 模型与连接 › 联网搜索');
    expect(body).not.toContain('- `web`:');
  } finally {
    await fixture.close();
  }
});
