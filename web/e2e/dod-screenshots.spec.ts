import { test, expect } from '@playwright/test';
import { rmSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startKernelFixture, type KernelFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T008/evidence/dod');

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
/// T008 #1（r4）：401 统一进令牌说明页——固定顺序：等第一次使用页 → 重启服务 → 点「接上」→ 断言令牌说明页。
/// Keyring 清空等价于「重新开始」：旧 cookie 失效，重启后第一次使用页的「接上」会拿到 401。
test('DoD7 令牌失效：第一次使用页 → 重启服务 → 接上 → 令牌说明页', async ({ page }) => {
  const fixture = await startKernelFixture({ emptyKeyring: true });
  try {
    await page.goto(urlOf(fixture));
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 30_000 });
    for (const name of ['keyring.age', 'identity.txt', 'config.json']) {
      rmSync(path.join(fixture.dataDir, name), { force: true });
    }
    await fixture.restart?.();
    await page.locator('#first-use-key').fill('sk-test-r4');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'token', { timeout: 30_000 });
    await expect(page.getByText('这个页面需要重新打开')).toBeVisible();
    await expect(page.getByText(/服务拒绝|401/)).toHaveCount(0);
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
