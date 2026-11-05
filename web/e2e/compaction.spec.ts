import { test, expect, type APIRequestContext } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

async function openAndSend(page: import('@playwright/test').Page, fixture: KernelFixture, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送' }).click();
  await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
}

async function followEvents(request: APIRequestContext, fixture: KernelFixture) {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  return (await response.json()).events as { type: string }[];
}

/// T007.8 清单 6：手动压缩 → 对话里出现分隔线，点开看摘要。
test('手动压缩：分隔线出现，点开是摘要', async ({ page, request }) => {
  const fixture = await startKernelFixture({ highUsage: true, contextWindow: 5000 });
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await openAndSend(page, fixture, '写点东西，我要手动压缩');
    // 底栏占用/压缩在悬停时才显示。
    await page.getByText('DeepSeek Chat').last().hover();
    const compact = page.getByRole('button', { name: '压缩', exact: true });
    await expect(compact).toBeEnabled({ timeout: 15_000 });
    await compact.click();
    const events = await followEvents(request, fixture);
    expect(events.filter((event) => event.type === 'compaction/summary').length).toBeGreaterThanOrEqual(1);
    const divider = page.locator('[data-kind="compaction"]').last();
    await expect(divider).toBeVisible({ timeout: 20_000 });
    await expect(divider.getByText('你手动', { exact: false })).toBeVisible();
    // T010.4：手动压缩是这个测试里最后一次模型请求，压缩后没有请求 → 不显示「占用」。
    await expect(divider.getByText('占用', { exact: false })).toHaveCount(0);
    await divider.getByRole('button').click();
    await expect(divider.getByText('（T007 e2e 假回放）', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});

/// T007.8 清单 6：自动压缩 → 分隔线出现（快满了，自动），点开是摘要。
test('自动压缩：分隔线出现', async ({ page, request }) => {
  const fixture = await startKernelFixture({ highUsage: true, contextWindow: 5000 });
  try {
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await openAndSend(page, fixture, '先把上下文撑大');
    await openAndSend(page, fixture, '再发一条，触发自动压缩');
    const events = await followEvents(request, fixture);
    expect(events.filter((event) => event.type === 'compaction/summary').length).toBeGreaterThanOrEqual(1);
    const divider = page.locator('[data-kind="compaction"]').last();
    await expect(divider).toBeVisible({ timeout: 30_000 });
    await expect(divider.getByText('快满了，自动', { exact: false })).toBeVisible();
    // T010.4：前=摘要请求 context÷窗口、后=压缩后第一条 assistant/message context÷窗口（窗口来自模型设置）。
    await expect(divider.getByText(/占用 \d+% → \d+%/)).toBeVisible();
  } finally {
    await fixture.close();
  }
});
