import { test, expect } from '@playwright/test';
import { renameSync } from 'node:fs';
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
/// T008 #1：令牌说明页用真能走到的路——页面开着 → Keyring 重新开始（含 cookie 签名钥匙）→ 重启服务 → 下一次调用 401。
test('DoD7 令牌失效：Keyring 重新开始并重启服务后页面 401', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    // 先挪走钥匙文件，让设置页出现「读不出」横幅，再点「重新开始」清空 Keyring。
    const identity = path.join(fixture.dataDir, 'identity.txt');
    const moved = path.join(fixture.dataDir, 'moved-identity.txt');
    renameSync(identity, moved);
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const banner = page.locator('[data-settings="keyring-broken"]');
    await expect(banner).toBeVisible({ timeout: 20_000 });
    await banner.getByRole('button', { name: '找不回了，重新开始' }).click();
    await page.getByRole('button', { name: '重新开始', exact: true }).click();
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await fixture.restart?.();
    // 新进程从空 Keyring 新建 cookie 签名钥匙；页面旧 cookie 在断线 recheck 时 401 → 令牌说明页。
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'token', { timeout: 45_000 });
    await expect(page.getByText('这个页面需要重新打开')).toBeVisible();
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
