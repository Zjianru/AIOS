import { test, expect, type APIRequestContext } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

async function open(page: import('@playwright/test').Page, fixture: KernelFixture) {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  await page.getByPlaceholder('和 Kind 说…').fill('你会失败吗？');
  await page.getByRole('button', { name: '发送' }).click();
}

async function followEvents(request: APIRequestContext, fixture: KernelFixture) {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  return (await response.json()).events as { type: string; data?: { delayMs?: number } }[];
}

/// T007.7 清单 5：重试两次后成功 → 页面出现「第 N 次重试」；日志有 llm/retry。
test('重试两次后成功：页面显示第几次重试、为什么', async ({ page, request }) => {
  const fixture = await startKernelFixture({ failure: 'retry-then-succeed' });
  try {
    await open(page, fixture);
    await expect(page.getByText(/次重试/)).toBeVisible({ timeout: 30_000 });
    // 第 3 次重试的退避是 2 秒：页面「X 秒后」必须等于 llm/retry.delayMs 折算值（camelCase）。
    await expect(page.getByText('2 秒后', { exact: false })).toBeVisible({ timeout: 40_000 });
    await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 30_000 });
    const events = await followEvents(request, fixture);
    const retries = events.filter((event) => event.type === 'llm/retry');
    expect(retries.length).toBeGreaterThanOrEqual(3);
    expect(
      retries.some((event) => Math.round((event.data?.delayMs ?? 0) / 1000) === 2),
    ).toBeTruthy();
    expect(retries.every((event) => typeof event.data?.delayMs === 'number')).toBeTruthy();
  } finally {
    await fixture.close();
  }
});

/// T007.7 清单 5：重试用完失败 → 按原因写失败块（模型），有「再发一次」。
test('模型重试用完：失败块按模型原因、可再发一次', async ({ page }) => {
  const fixture = await startKernelFixture({ failure: 'model' });
  try {
    await open(page, fixture);
    await expect(page.getByText('这一轮没做完', { exact: false })).toBeVisible({
      timeout: 40_000,
    });
    await expect(page.getByRole('button', { name: '再发一次' })).toBeVisible();
    await expect(page.getByRole('button', { name: /去设置/ })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T007.7 清单 5 + T008.4：401 凭证失败 → 失败块按凭证原因，并给「去设置」。
test('凭证 401：失败块按凭证原因、不放去设置', async ({ page }) => {
  const fixture = await startKernelFixture({ failure: 'credential' });
  try {
    await open(page, fixture);
    await expect(page.getByText('这一轮没做完', { exact: false })).toBeVisible({
      timeout: 20_000,
    });
    await expect(page.getByText('换一把密钥', { exact: false })).toBeVisible();
    await expect(page.getByRole('button', { name: '再发一次' })).toBeVisible();
    await expect(page.getByRole('button', { name: /去设置/ })).toBeVisible();
  } finally {
    await fixture.close();
  }
});
