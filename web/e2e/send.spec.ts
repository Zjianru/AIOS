import { test, expect } from '@playwright/test';
import { startKernelFixture, type KernelFixture } from './service-fixture';

let fixture: KernelFixture;

test.beforeAll(async () => {
  fixture = await startKernelFixture({ toolTurn: true });
});

test.afterAll(async () => {
  await fixture?.close();
});

/// T007.4 清单 2：发一条 → 用户消息出现 → 工具逐条出现（read hello.txt）→
/// 流式回答（假 DeepSeek 两次 delta）→ 跟到最新。
test('发一条：工具逐条出现、流式回答、跟到最新', async ({ page }) => {
  await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
  const input = page.getByPlaceholder('和 Kind 说…');
  await expect(input).toBeVisible();
  await input.fill('请读一下 hello.txt 再回答');
  await page.getByRole('button', { name: '发送' }).click();
  // 流式回答收尾：最终文本出现在这一轮里。
  await expect(page.getByText('（T007 e2e 假回放）')).toBeVisible({ timeout: 15_000 });
  // T013.5：工作过程收成「已完成，用时 X」，点开看到组和行；点行出 read 卡片。
  const process = page.locator('[data-kind="process"]').last();
  await expect(process).toBeVisible({ timeout: 15_000 });
  await expect(process.locator('button').first()).toContainText(/已完成，用时/, { timeout: 15_000 });
  await process.locator('button').first().click();
  const group = process.locator('[data-kind="process-group"] button').first();
  await expect(group).toContainText(/已读取文件|正在读取文件/, { timeout: 15_000 });
  await group.click();
  const row = process.locator('[data-kind="tool-row"]').first();
  await expect(row).toContainText('读取');
  await row.locator('button').first().click();
  // read 卡片：带行号的正文（fixture 的 hello.txt）。
  await expect(process.getByText('hello.txt', { exact: false }).first()).toBeVisible({ timeout: 15_000 });
});
