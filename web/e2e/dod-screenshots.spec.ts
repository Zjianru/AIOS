import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T007/evidence/dod');

test.describe.configure({ mode: 'serial' });
test.setTimeout(60_000);

function urlOf(fixture: KernelFixture, token = fixture.token): string {
  return `${fixture.baseUrl}/?token=${encodeURIComponent(token)}#kind`;
}

async function shot(page: import('@playwright/test').Page, name: string) {
  await mkdir(evidence, { recursive: true });
  await page.screenshot({ path: path.join(evidence, name), fullPage: true });
}

/// DoD7：五种说明页各一张截图（造法见 evidence/dod/README.md）。
/// T007 #1：令牌失效用真能走到的路——页面开着 → 重启服务（新进程新 cookie 钥匙）→ 下一次调用 401。
test('DoD7 令牌失效：页面开着重启服务后下一次请求 401', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });
    await fixture.restart?.();
    // 页面开着 → 重启服务（新进程=新 cookie 钥匙）→ 断线 recheck 的下一次 /api 收到 401 → 令牌说明页。
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'token', { timeout: 40_000 });
    await expect(page.getByText('这个地址已经失效')).toBeVisible();
    await shot(page, 'dod7-1-token.png');
  } finally {
    await fixture.close();
  }
});

test('DoD7 对话丢失', async ({ page }) => {
  const fixture = await startKernelFixture({ rootFault: 'lost' });
  try {
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'lost', { timeout: 20_000 });
    await shot(page, 'dod7-2-lost.png');
  } finally {
    await fixture.close();
  }
});

test('DoD7 日志读不懂', async ({ page }) => {
  const fixture = await startKernelFixture({ rootFault: 'corrupt' });
  try {
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'broken', { timeout: 20_000 });
    await shot(page, 'dod7-3-broken.png');
  } finally {
    await fixture.close();
  }
});

test('DoD7 服务连不上', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.route(/\/api(\?|$)/, (route) => route.abort());
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'unreachable', { timeout: 20_000 });
    await shot(page, 'dod7-4-unreachable.png');
  } finally {
    await fixture.close();
  }
});

test('DoD7 对话被占用', async ({ page }) => {
  const fixture = await startKernelFixture({ busyRoot: true });
  try {
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'busy', { timeout: 30_000 });
    await shot(page, 'dod7-5-busy.png');
  } finally {
    await fixture.close();
  }
});
