import { renameSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from '@playwright/test';
import { startRealDodFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T008/evidence/dod');

test.skip(!process.env.DEEPSEEK_API_KEY, '需要运行时 DEEPSEEK_API_KEY（真 key DoD 3）');

/** T008.F2：只断言最终 assistant 回答，不把工具/用户消息算进来。 */
async function expectFinalAnswer(page: Page, text: string): Promise<void> {
  await expect(page.locator('[data-kind="assistant-answer"]').last()).toContainText(text);
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0);
}

test('DoD3 真 key：页面第一次填 key → 接上 → 空对话 → read 只读 /tmp/aios-t008-dod/work 并回答', async ({ page }) => {
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
    await expect(page.getByText('已保存到 Keyring')).toBeVisible({ timeout: 120_000 });
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 60_000 });
    await page.screenshot({ path: path.join(evidence, 'dod3-empty.png'), fullPage: true });

    await page.getByPlaceholder('和 Kind 说…').fill(
      '请只用 read 工具、只读地查看 /tmp/aios-t008-dod/work/hello.txt 的内容，把文件内容原样回复我。不要写任何文件。',
    );
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.getByText('DOD-T008-OK', { exact: false })).toBeVisible({ timeout: 180_000 });
    await expectFinalAnswer(page, 'DOD-T008-OK');
    await page.screenshot({ path: path.join(evidence, 'dod3-answer.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});

// DoD4/5（删 key 后失败块说凭证、挪走钥匙文件后说 Keyring、指回恢复、外泄扫描）下一窗口补；本文件先留可跑的 DoD3 骨架。

test('DoD4 真 key：删 key 凭证失败、挪钥匙 Keyring 失败、指回恢复', async ({ page }) => {
  test.setTimeout(300_000);
  const fixture = await startRealDodFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await expect(page.locator('[data-first-use="connect"]')).toBeVisible({ timeout: 30_000 });
    await page.locator('#first-use-key').fill(process.env.DEEPSEEK_API_KEY ?? '');
    await page.getByRole('button', { name: '接上', exact: true }).click();
    await expect(page.getByText('已保存到 Keyring')).toBeVisible({ timeout: 120_000 });
    await page.getByRole('button', { name: '开始', exact: true }).click();
    await expect(page.locator('[data-kind="empty"]')).toBeVisible({ timeout: 60_000 });

    // 设置页撤销 key，再发：失败块说凭证。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.getByRole('button', { name: '撤销密钥', exact: true }).click();
    await page.getByRole('button', { name: '撤销', exact: true }).click();
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="failure"]')).toBeVisible({ timeout: 120_000 });
    await expect(page.getByText('换一把密钥', { exact: false })).toBeVisible();
    await page.screenshot({ path: path.join(evidence, 'dod4-credential.png'), fullPage: true });

    // 设置页换回真 key。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.locator('#key-deepseek').fill(process.env.DEEPSEEK_API_KEY ?? '');
    // 页面上还有一个「保存」在联网搜索一节（填 Exa 的 key）；这里限定连接编辑卡。
    await page
      .locator('[data-settings="editor"]')
      .getByRole('button', { name: '保存', exact: true })
      .click();
    await expect(page.locator('[data-settings="editor"]')).toHaveCount(0, { timeout: 20_000 });
    // 保存带 key：connection-save 顺手拉厂商列表。
    await expect(page.getByText(/已拿到模型列表/)).toBeVisible({ timeout: 120_000 });
    await page.screenshot({ path: path.join(evidence, 'dod4-settings-issues.png'), fullPage: true });

    // 挪走钥匙文件，再发：失败块说 Keyring。
    const identity = path.join(fixture.dataDir, 'identity.txt');
    const moved = path.join(fixture.dataDir, 'moved-identity.txt');
    renameSync(identity, moved);
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.getByPlaceholder('和 Kind 说…').fill('你好');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    await expect(page.locator('[data-kind="failure"]')).toBeVisible({ timeout: 120_000 });
    await expect(page.getByText('AIOS 不会改用别处的密钥', { exact: false })).toBeVisible();
    await page.screenshot({ path: path.join(evidence, 'dod4-keyring.png'), fullPage: true });

    // 设置页横幅指回钥匙文件，再发一条恢复。
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const banner = page.locator('[data-settings="keyring-broken"]');
    await expect(banner).toBeVisible({ timeout: 20_000 });
    await banner.getByRole('button', { name: '指到它现在的位置' }).click();
    await banner.getByPlaceholder('钥匙文件现在的完整路径，比如 ~/Backups/keyring.key').fill(moved);
    await banner.getByRole('button', { name: '用这个' }).click();
    await expect(page.locator('[data-settings="keyring-broken"]')).toHaveCount(0, { timeout: 20_000 });
    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.getByPlaceholder('和 Kind 说…').fill('只回复 DOD4-OK');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    // L3：只断言最后一条 assistant 回答；用户消息里也含 DOD4-OK，不能算。
    const recovered = page.locator('[data-kind="assistant-answer"]').last();
    await expect(recovered).toContainText('DOD4-OK', { timeout: 180_000 });
    await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0);
    await page.screenshot({ path: path.join(evidence, 'dod4-recovered.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});
