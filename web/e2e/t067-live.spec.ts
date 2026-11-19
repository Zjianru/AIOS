import { expect, test, type Page } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/** T067.4：页面收到事件才重读；空闲不发请求；根 WS 转发 task-changed。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

/** 数服务端读请求：method 由 /api 的 POST body 决定。 */
function countApi(page: Page, method: string): { count: () => number; times: () => number[] } {
  let count = 0;
  const times: number[] = [];
  page.on('request', (request) => {
    if (!request.url().endsWith('/api')) return;
    try {
      const body = JSON.parse(request.postData() ?? '{}') as { method?: string };
      if (body.method === method) {
        count += 1;
        times.push(Date.now());
      }
    } catch {
      // 不是 JSON 的请求不算。
    }
  });
  return { count: () => count, times: () => times };
}

test('T067 工作页空闲不轮询：打开读一次，随后 5 秒 0 个 work 请求', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture, '#kind'));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });
    const workReads = countApi(page, 'work');
    await page.goto(urlOf(fixture, '#work'));
    await expect(page.locator('[data-kind="work-board"]')).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(2_500);
    const baseline = workReads.count();
    await page.waitForTimeout(5_000);
    expect(workReads.count()).toBe(baseline);
  } finally {
    await fixture.close();
  }
});

test('T067 日历空闲不轮询：打开读一次，随后 5 秒 0 个 calendar-read', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    const reads = countApi(page, 'calendar-read');
    await page.goto(urlOf(fixture, '#work?wv=cal'));
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(2_500);
    const baseline = reads.count();
    await page.waitForTimeout(5_000);
    expect(reads.count()).toBe(baseline);
  } finally {
    await fixture.close();
  }
});

test('T067 Kind 任务卡空闲不轮询：打开读一次，随后 4 秒 0 个 task-read', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture, '#kind'));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 60_000 });
    const reads = countApi(page, 'task-read');
    await page.reload();
    await expect(page.locator('[data-kind="task-result"]').last()).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(2_500);
    const baseline = reads.count();
    await page.waitForTimeout(4_000);
    expect(reads.count()).toBe(baseline);
  } finally {
    await fixture.close();
  }
});

test('T067 任务事实变了根 WS 收到 task-changed，工作页立即重读', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture, '#kind'));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card.locator('[data-kind="task-accept"]')).toBeVisible({ timeout: 60_000 });

    // 页面另开一根根 WS，专收 task-changed 帧（页面自己的 live 连接照常）。
    await page.evaluate(() => {
      (window as unknown as { __taskChanged: string[]; __taskChangedOpen: boolean }).__taskChanged = [];
      (window as unknown as { __taskChangedOpen: boolean }).__taskChangedOpen = false;
      const socket = new WebSocket(
        'ws://' + location.host + '/ws?lastSeq=' + Number.MAX_SAFE_INTEGER,
      );
      socket.onopen = () => {
        (window as unknown as { __taskChangedOpen: boolean }).__taskChangedOpen = true;
      };
      socket.onmessage = (message) => {
        const frame = JSON.parse(String(message.data)) as { type?: string; taskId?: string };
        if (frame.type === 'task-changed' && frame.taskId) {
          (window as unknown as { __taskChanged: string[] }).__taskChanged.push(frame.taskId);
        }
      };
    });

    await expect
      .poll(
        async () =>
          page.evaluate(
            () => (window as unknown as { __taskChangedOpen?: boolean }).__taskChangedOpen === true,
          ),
        { timeout: 5_000 },
      )
      .toBe(true);
    const workReads = countApi(page, 'work');
    await card.locator('[data-kind="task-accept"]').click();
    await expect
      .poll(async () => page.evaluate(() => (window as unknown as { __taskChanged: string[] }).__taskChanged.length), {
        timeout: 5_000,
      })
      .toBeGreaterThan(0);
    // 接受以后切到工作页：打开就读一次；再等 5 秒没有轮询。
    await page.goto(urlOf(fixture, '#work'));
    await expect(page.locator('[data-kind="work-board"]')).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(2_500);
    const baseline = workReads.count();
    await page.waitForTimeout(5_000);
    expect(workReads.count()).toBe(baseline);
  } finally {
    await fixture.close();
  }
});
