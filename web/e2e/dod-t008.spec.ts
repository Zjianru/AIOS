import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect } from '@playwright/test';
import { startRealDodFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T008/evidence/dod');

test.skip(!process.env.DEEPSEEK_API_KEY, '需要运行时 DEEPSEEK_API_KEY（真 key DoD 3）');

test('DoD3 真 key：页面第一次填 key → 接上 → 空对话 → bash 只读 /tmp/aios-t008-dod/work 并回答', async ({ page }) => {
  test.setTimeout(240_000);
  const fixture = await startRealDodFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 30_000 });
    const input = page.locator('#first-use-key');
    await expect(input).toHaveAttribute('type', 'password');
    await input.fill(process.env.DEEPSEEK_API_KEY ?? '');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    await expect(page.getByText('连接可用')).toBeVisible({ timeout: 120_000 });
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 60_000 });
    await page.screenshot({ path: path.join(evidence, 'dod3-empty.png'), fullPage: true });

    await page.getByPlaceholder('和 Kind 说…').fill(
      '请只用 bash 工具、只读地查看 /tmp/aios-t008-dod/work/hello.txt 的内容，把文件内容原样回复我。不要写任何文件。',
    );
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('DOD-T008-OK', { exact: false })).toBeVisible({ timeout: 180_000 });
    await page.screenshot({ path: path.join(evidence, 'dod3-answer.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});

// DoD4/5（删 key 后失败块说凭证、挪走钥匙文件后说 Keyring、指回恢复、外泄扫描）下一窗口补；本文件先留可跑的 DoD3 骨架。
