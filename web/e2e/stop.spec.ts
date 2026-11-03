import { test, expect, type APIRequestContext } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture({ slowMs: 3000 });
});

test.afterAll(async () => {
  await fixture?.close();
});

async function abortedTurns(request: APIRequestContext): Promise<number> {
  const response = await request.post(
    `${fixture.baseUrl}/api?token=${encodeURIComponent(fixture.token)}`,
    { data: { method: 'follow', lastSeq: 0 } },
  );
  const body = await response.json();
  return (body.events as { type: string; data?: { reason?: { kind?: string } } }[]).filter(
    (event) => event.type === 'turn/end' && event.data?.reason?.kind === 'aborted',
  ).length;
}

/// T007.5 清单 3：停止按钮和 Esc 各一次 → 服务收到取消（turn/end aborted）→
/// 页面留下一行「停下了这一轮」。
test('停止：按钮和 Esc 各一次，服务收到取消', async ({ page, request }) => {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  const input = page.getByPlaceholder('和 Kind 说…');

  await input.fill('先停一下（按钮）');
  await page.getByRole('button', { name: '发送' }).click();
  await expect(page.getByText('先停一下（按钮）')).toBeVisible({ timeout: 15_000 });
  const stop = page.getByRole('button', { name: '停止' });
  await expect(stop).toBeVisible({ timeout: 10_000 });
  await stop.click();
  await expect(page.getByText('停下了这一轮', { exact: false })).toHaveCount(1, {
    timeout: 15_000,
  });

  await input.fill('再停一下（Esc）');
  await page.getByRole('button', { name: '发送' }).click();
  await expect(page.getByText('再停一下（Esc）')).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole('button', { name: '停止' })).toBeVisible({ timeout: 10_000 });
  await page.keyboard.press('Escape');
  await expect(page.getByText('停下了这一轮', { exact: false })).toHaveCount(2, {
    timeout: 15_000,
  });

  await expect.poll(() => abortedTurns(request), { timeout: 15_000 }).toBeGreaterThanOrEqual(2);
});
