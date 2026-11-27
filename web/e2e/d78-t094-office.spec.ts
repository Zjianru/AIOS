//! T094.5 D78：真 key（MiniMax）读 xlsx 结构、插件关/开、插件开关卡。
//! 无 key 时跳过；key 从环境取，不落文件/命令行/证据。
import { copyFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { expect, test, type Page } from '@playwright/test';
import { apiPost, startD78KernelFixture } from './service-fixture';

test.skip(!process.env.MINIMAX_API_KEY, '需要 MINIMAX_API_KEY（D78）');
test.setTimeout(420_000);

const FIXTURE = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  'fixtures',
  'office',
  'sample.xlsx',
);

const input = (page: Page) => page.locator('[data-kind="composer-input"]');
const sendButton = (page: Page) => page.locator('[data-kind="send"]');

async function send(page: Page, text: string) {
  await input(page).fill(text);
  await sendButton(page).click();
}

async function waitFinished(page: Page) {
  await expect(page.getByRole('button', { name: /已完成|已派出|停下了/ }).last()).toBeVisible({
    timeout: 240_000,
  });
}

async function answerText(page: Page): Promise<string> {
  return (await page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last().innerText()).trim();
}

async function enableOffice(request: any, fixture: { baseUrl: string; token: string }, enabled: boolean) {
  const response = await apiPost(request, fixture, {
    method: 'plugin_set',
    id: 'office',
    enabled,
  });
  expect(response.ok()).toBeTruthy();
}

async function officePath(fixture: { dataDir: string }): Promise<string> {
  const target = path.join(fixture.dataDir, 'sample.xlsx');
  await copyFile(FIXTURE, target);
  return target;
}

test('D78 Office：插件开时 Kind 读 xlsx 说结构', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await enableOffice(request, fixture, true);
    const file = await officePath(fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(input(page)).toBeVisible({ timeout: 60_000 });
    await send(
      page,
      '请读这个 xlsx：' + file + '。只按读到的说：有哪些工作表，A1 和 B1 分别是什么？',
    );
    await waitFinished(page);
    const answer = await answerText(page);
    expect(answer).toContain('汇总');
    expect(answer).toContain('名字');
  } finally {
    await fixture.close();
  }
});

test('D78 Office：插件关时 Kind 不编 xlsx 内容', async ({ page, request }) => {
  const fixture = await startD78KernelFixture();
  try {
    await enableOffice(request, fixture, false);
    const file = await officePath(fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(input(page)).toBeVisible({ timeout: 60_000 });
    await send(page, '请读这个 xlsx：' + file + '，告诉我第一张表 A1 写的是什么。');
    await waitFinished(page);
    const answer = await answerText(page);
    expect(answer).not.toContain('汇总');
    expect(answer).not.toContain('名字');
    expect(answer).toMatch(/插件|Office|读不了|看不了|二进制|不支持/);
  } finally {
    await fixture.close();
  }
});

test('D78 Office：说「打开 Office 插件」出确认卡，确认后下一轮 office_read 可用', async ({
  page,
  request,
}) => {
  const fixture = await startD78KernelFixture();
  try {
    await enableOffice(request, fixture, false);
    const file = await officePath(fixture);
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    await expect(input(page)).toBeVisible({ timeout: 60_000 });
    await send(page, '打开 Office 插件。');
    await waitFinished(page);
    const card = page.locator('[data-kind="confirm-card"]').last();
    await expect(card).toBeVisible({ timeout: 60_000 });
    await expect(card).toContainText('Office');
    await expect(card).toContainText('影响');
    await expect(card).toContainText('插件');
    const options = card.locator('[data-action="card-option"]');
    if ((await options.count()) > 0) {
      await options.first().click();
      await expect(options.first()).toHaveAttribute('aria-checked', 'true');
    }
    await card.locator('[data-action="card-confirm"]').click();
    await expect(card).toHaveAttribute('data-state', 'done', { timeout: 60_000 });
    const list = await apiPost(request, fixture, { method: 'plugin_list' });
    const body = (await list.json()) as {
      plugins?: { id: string; enabled: boolean }[];
      data?: { plugins?: { id: string; enabled: boolean }[] };
    };
    const plugins = body.plugins ?? body.data?.plugins ?? [];
    expect(plugins.find((plugin) => plugin.id === 'office')?.enabled).toBe(true);

    await send(page, '现在读 ' + file + ' 的 A1 和 B1，各是什么？只按读到的回答。');
    await waitFinished(page);
    const answer = await answerText(page);
    expect(answer).toContain('名字');
  } finally {
    await fixture.close();
  }
});
