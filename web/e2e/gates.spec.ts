import { test, expect } from '@playwright/test';
import { rmSync } from 'node:fs';
import path from 'node:path';
import { startKernelFixture, type KernelFixture } from './service-fixture';

function urlOf(fixture: KernelFixture, token = fixture.token): string {
  return `${fixture.baseUrl}/?token=${encodeURIComponent(token)}#kind`;
}

test('令牌失效：说明页', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture, 'wrong-token'));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'token', { timeout: 20_000 });
    await expect(gate.getByText('这个地址已经失效')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('对话丢失：说明页 + 再检查一次修好后回对话', async ({ page }) => {
  const fixture = await startKernelFixture({ rootFault: 'lost' });
  try {
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'lost', { timeout: 20_000 });
    await expect(gate.getByText('Kind 的对话找不到了')).toBeVisible();
    rmSync(path.join(fixture.dataDir, 'kind-root.json'));
    await gate.getByRole('button', { name: '再检查一次' }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('日志读不懂：说明页（带行号）', async ({ page }) => {
  const fixture = await startKernelFixture({ rootFault: 'corrupt' });
  try {
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'broken', { timeout: 20_000 });
    await expect(gate.getByText('Kind 的对话日志读不懂')).toBeVisible();
    await expect(gate.getByText('第', { exact: false })).toBeVisible();
    await expect(gate.getByText('不是一条完整的记录', { exact: false })).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('服务连不上：说明页 + 现在重试', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.route(/\/api\?/, (route) => route.abort());
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'unreachable', { timeout: 20_000 });
    await expect(gate.getByText('连不上内核')).toBeVisible();
    await page.unroute(/\/api\?/);
    await gate.getByRole('button', { name: /现在重试/ }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 20_000 });
  } finally {
    await fixture.close();
  }
});

test('对话被占用：说明页', async ({ page }) => {
  const fixture = await startKernelFixture({ busyRoot: true });
  try {
    await page.goto(urlOf(fixture));
    const gate = page.locator('[data-kind="kernel-gate"]');
    await expect(gate).toHaveAttribute('data-state', 'busy', { timeout: 30_000 });
    await expect(gate.getByText('正被别的程序占用')).toBeVisible();
    await expect(gate.getByText('另一个 AIOS 进程正在写这条对话')).toBeVisible();
  } finally {
    await fixture.close();
  }
});
