import { test, expect, type APIRequestContext } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeEach(async () => {
  fixture = await startKernelFixture({ questionDegrade: true });
});

test.afterEach(async () => {
  await fixture?.close();
});

/** 假回放里那道题：25 字小标题、2233 字的问题、选项里有一个空 label（内核保留）和一个缺 label（内核丢掉）。 */
const HEADER = '这是一个特别长的小标题超过二十个字所以不该显示出来';
const TAIL = '长问题结尾标记';
const urlOf = () =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function askedOptions(request: APIRequestContext): Promise<string[]> {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  expect(response.ok()).toBeTruthy();
  const body = (await response.json()) as {
    events: { type: string; data?: { options?: string[] } }[];
  };
  return body.events
    .filter((event) => event.type === 'question/asked')
    .flatMap((event) => event.data?.options ?? []);
}

/// 清单 10：降级在页面上生效——卡片不报错、超长小标题截断显示、坏选项不显示、问题能展开、提交收成回执、刷新后仍是回执。
test('提问卡降级：超长小标题、空 label 的选项、2000 字的问题', async ({ page, request }) => {
  await page.goto(urlOf());
  await page.getByPlaceholder('和 Kind 说…').fill('问我一个很难的问题');
  await page.getByRole('button', { name: '发送', exact: true }).click();

  const card = page.locator('[data-kind="question"]');
  await expect(card).toBeVisible({ timeout: 20_000 });

  // 小标题超过 20 字：截断到 20 字显示，不整段消失；问题照常显示。
  const headerShown = '这是一个特别长的小标题超过二十个字所以不';
  await expect(card.getByText(headerShown, { exact: true }).first()).toBeVisible();
  await expect(page.getByText(HEADER, { exact: false })).toHaveCount(0);
  await expect(card.getByText('很长的问题', { exact: false })).toBeVisible();

  // 选项：事件里是三项（内核保留了空 label 的那项），页面只显示两项，顺序不变。
  expect(await askedOptions(request)).toEqual(['第一项', '', '最后一项']);
  await expect(card.locator('[role="radio"]')).toHaveCount(2);
  await expect(card.getByText('第一项', { exact: true })).toBeVisible();
  await expect(card.getByText('最后一项', { exact: true })).toBeVisible();
  await expect(card.getByText('缺 label 的项', { exact: false })).toHaveCount(0);

  // 2000 字的问题：先折起来，点「展开全文」看得到结尾，原文不丢。
  await expect(page.getByText(TAIL, { exact: false })).toHaveCount(0);
  await card.getByRole('button', { name: '展开全文' }).click();
  await expect(card.getByText(TAIL, { exact: false })).toBeVisible();

  // 提交：收成回执，不能再改。
  await card.getByRole('radio', { name: /第一项/ }).click();
  await card.getByRole('button', { name: '提交回答' }).click();
  await expect(page.getByText('（e2e 假回放）')).toBeVisible({ timeout: 20_000 });
  const closed = page.locator('[data-kind="question-closed"]');
  await expect(closed).toBeVisible();
  await expect(closed.getByText('你回答了 Kind 的 1 个问题')).toBeVisible();
  await expect(closed.getByText('第一项', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '提交回答' })).toHaveCount(0);

  // 刷新后仍是回执，不能再改。
  await page.reload();
  const closedAgain = page.locator('[data-kind="question-closed"]');
  await expect(closedAgain).toBeVisible({ timeout: 20_000 });
  await expect(closedAgain.getByText('你回答了 Kind 的 1 个问题')).toBeVisible();
  await expect(page.getByRole('button', { name: '提交回答' })).toHaveCount(0);
  await expect(page.locator('[data-kind="question"]')).toHaveCount(0);
});
