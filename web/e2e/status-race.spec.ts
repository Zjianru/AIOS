import { test, expect, type Page, type Route } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T021：页面的 status 刷新是异步的（首屏一次、turn/end 一次、recheck 一次）。
 * 旧请求的回包后到会覆盖新状态——T018.1 诊断日志：新的 turns=1/contextUsed=4500 先回，
 * 首屏的 turns=0/contextUsed=0 后回，结果压缩按钮永久禁用、上下文圆环 0%。
 *
 * 这里扣住首屏 status 的**回包**（服务端在发消息之前就算好了，所以是旧状态），
 * 等这一轮的新 status 落地后再放行，断言新状态没有被旧回包覆盖。
 * 注意：扣的是回包不是请求——晚发请求只会让服务端算出新状态，复现不了这个竞态。
 */

interface StatusHold {
  /** 首屏 status 的回包已抓在手里（还没写回页面）。 */
  held: Promise<void>;
  /** 放行旧回包。 */
  release: () => void;
  /** 旧回包已经写回页面；之后断言「没被覆盖」才有意义。 */
  delivered: Promise<void>;
}

async function holdFirstStatus(page: Page): Promise<StatusHold> {
  let statusCount = 0;
  let release = (): void => {};
  let markHeld = (): void => {};
  let markDelivered = (): void => {};
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  const held = new Promise<void>((resolve) => {
    markHeld = resolve;
  });
  const delivered = new Promise<void>((resolve) => {
    markDelivered = resolve;
  });

  await page.route('**/api', async (route: Route) => {
    const request = route.request();
    let method: unknown;
    try {
      method = (JSON.parse(request.postData() ?? '{}') as { method?: unknown }).method;
    } catch {
      method = undefined;
    }
    if (method !== 'status') {
      await route.continue();
      return;
    }
    statusCount += 1;
    if (statusCount > 1) {
      // 后面的 status 照常走：这一轮结束时的那次才是「新」状态。
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const body = await response.text();
    markHeld();
    await released;
    await route.fulfill({ status: response.status(), headers: response.headers(), body });
    markDelivered();
  });
  return { held, release: () => release(), delivered };
}

test('页面一打开就发：旧 status 晚到不覆盖新状态', async ({ page }) => {
  const fixture = await startKernelFixture({ highUsage: true, contextWindow: 5000 });
  try {
    const hold = await holdFirstStatus(page);
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    // 不等首屏 status 落地（它正被扣着），页面能输入就立刻发。
    await hold.held;
    await page.getByPlaceholder('和 Kind 说…').fill('刚打开就发，不等首屏 status');
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByText('（T007 e2e 假回放）').last()).toBeVisible({ timeout: 30_000 });
    // 这一轮结束（turn/end）后页面再刷一次 status：新回包 turns=1 / contextUsed=4500。
    await expect(page.locator('[data-kind="stop"]')).toHaveCount(0, { timeout: 60_000 });
    const meter = page.locator('[data-kind="context-meter"]');
    await expect(meter).toContainText('90%', { timeout: 15_000 });
    await meter.click();
    const compact = page.locator('[data-kind="compact"]');
    await expect(compact).toBeEnabled({ timeout: 15_000 });

    // 放行旧 status，等它真的写回页面；再等两帧绘制，让回包被解析、渲染完（被覆盖的话这里已经看得见）。
    hold.release();
    await hold.delivered;
    await page.evaluate(
      () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
    );
    await expect(meter).toContainText('90%');
    await expect(compact).toBeEnabled();
  } finally {
    await fixture.close();
  }
});
