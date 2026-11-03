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
  // 工具逐条：展开「工作过程」看 read 的真实结果（fixture 目录里的 hello.txt）。
  await page.getByRole('button', { name: /工作过程/ }).click();
  await expect(page.getByText('调了 1 次工具', { exact: false })).toBeVisible({
    timeout: 15_000,
  });
});
