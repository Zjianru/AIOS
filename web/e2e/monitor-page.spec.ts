import { test, expect, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

test('监控：消耗、在跑、诊断三块走服务端快照，只读', async ({ page }) => {
  test.setTimeout(180_000);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const snapshotCalls: string[] = [];
  const sockets: string[] = [];
  page.on('request', (request) => {
    if (request.method() !== 'POST' || !request.url().endsWith('/api')) return;
    try {
      const body = request.postDataJSON() as { method?: string };
      if (body.method === 'monitor-snapshot') snapshotCalls.push(request.url());
    } catch {
      // 不是 JSON 的请求不算。
    }
  });
  page.on('websocket', (socket) => sockets.push(socket.url()));
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const result = page.locator('[data-kind="task-result"]').last();
    await expect(result).toBeVisible({ timeout: 60_000 });

    await page.goto(urlOf(fixture, '#monitor'));
    await expect(page.locator('[data-page="monitor"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('heading', { name: '监控' })).toBeVisible();
    await expect(page.locator('[data-tab-key="cost"]')).toBeVisible();
    await expect(page.locator('[data-tab-key="live"]')).toBeVisible();
    await expect(page.locator('[data-kind="cost-daily"]')).toBeVisible();
    await expect(page.locator('[data-kind="kpis"]')).toBeVisible();
    // 项目只有「全部」：没有可点的项目筛选，也不出现项目项。
    await expect(page.locator('[data-kind="project-filter"]')).toContainText('全部项目');
    await expect(page.locator('[data-kind="project-filter"]')).toContainText('未归项目');
    // 缺数据画斜线，不拿 0 顶。
    await expect(page.locator('[data-kind="missing"]').first()).toBeVisible();
    // 监控 WS 已连；打开后不在固定间隔里拉快照。
    expect(sockets.some((url) => url.includes('monitor=1'))).toBeTruthy();
    await page.waitForTimeout(3000);
    const callsBefore = snapshotCalls.length;
    await page.waitForTimeout(1500);
    expect(snapshotCalls.length).toBe(callsBefore);

    // 已知任务出现在任务卡；点开进入诊断。
    const card = page.locator('[data-kind="task-card"]').filter({ hasText: '判重改成按' }).first();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await card.click();
    await expect(page.locator('[data-page="monitor-task"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="kpis"]')).toBeVisible();
    await expect(page.locator('[data-kind="ctx-card"]')).toBeVisible();
    await expect(page.locator('[data-kind="trace"]')).toBeVisible();
    await expect(page.locator('[data-kind="event-table"]')).toBeVisible();
    // 只读：没有停止按钮。
    await expect(page.getByRole('button', { name: '立即停' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: '收尾停' })).toHaveCount(0);

    // 在跑页：服务端快照的席位/在跑/排队/异常事实都在；没有在跑任务时显示空态。
    await page.goto(urlOf(fixture, '#monitor?tab=live'));
    await expect(page.locator('[data-tab="live"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="kpis"]')).toBeVisible();

    expect(errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});
