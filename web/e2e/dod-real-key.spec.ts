import { test, expect } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRealKernelFixture } from './service-fixture';

const evidence =
  process.env.AIOS_DOD_EVIDENCE ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../aios-work/T007/evidence/dod');

test.skip(!process.env.DEEPSEEK_API_KEY, '需要 DEEPSEEK_API_KEY（D34）');
test.setTimeout(240_000);

/// DoD4：真 key 基础——模型用 bash 只读 /tmp/aios-t007-dod/work 并回答；截图+日志比对。
test('DoD4 真 key：bash 只读 /tmp/aios-t007-dod/work 并回答', async ({ page, request }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill(
      '请只用 bash 工具、只读地查看 /tmp/aios-t007-dod/work/hello.txt 的内容，把文件内容原样回复我。不要写任何文件、不要安装。',
    );
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByText('DOD-4-OK', { exact: false })).toBeVisible({ timeout: 180_000 });
    // 等这一轮的 turn/end 落到日志（流式文本先到，日志后写），再做截图与比对。
    const followFor = async () => {
      const response = await request.post(
        `${fixture.baseUrl}/api?token=${encodeURIComponent(fixture.token)}`,
        { data: { method: 'follow', lastSeq: 0 } },
      );
      return (await response.json()).events as { type: string }[];
    };
    await expect
      .poll(async () => (await followFor()).filter((event) => event.type === 'turn/end').length, {
        timeout: 90_000,
      })
      .toBeGreaterThanOrEqual(1);
    await page.screenshot({ path: path.join(evidence, 'dod4-answer.png'), fullPage: true });
    const response = await request.post(
      `${fixture.baseUrl}/api?token=${encodeURIComponent(fixture.token)}`,
      { data: { method: 'follow', lastSeq: 0 } },
    );
    const events = (await response.json()).events as { type: string }[];
    const summary = {
      pageTurnsVisible: await page.getByText('工作过程', { exact: false }).count(),
      logTurns: events.filter((event) => event.type === 'turn/end').length,
      logUserMessages: events.filter((event) => event.type === 'user/message').length,
      token: '<redacted>',
    };
    await writeFile(path.join(evidence, 'dod4-compare.json'), JSON.stringify(summary, null, 2));
    expect(summary.logTurns).toBeGreaterThanOrEqual(1);
  } finally {
    await fixture.close();
  }
});

/// DoD5：真 key 停止（Esc）——跑到工具时停下，页面留「停下了」。
test('DoD5 真 key：跑到工具时 Esc 停下', async ({ page }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    await page.getByPlaceholder('和 Kind 说…').fill(
      '请必须调用 bash 工具运行前台命令：sleep 10；等它结束后回复我「完成」。',
    );
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByRole('button', { name: '停止' })).toBeVisible({ timeout: 30_000 });
    await page.keyboard.press('Escape');
    await expect(page.getByText('停下了这一轮', { exact: false })).toBeVisible({
      timeout: 30_000,
    });
    await page.screenshot({ path: path.join(evidence, 'dod5-stop.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});

/// DoD5：真 key 两题整批作答，然后整批不答。
test('DoD5 真 key：两题整批作答 + 整批不答', async ({ page }) => {
  const fixture = await startRealKernelFixture();
  try {
    await mkdir(evidence, { recursive: true });
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    const input = page.getByPlaceholder('和 Kind 说…');
    const prompt =
      '必须调用 ask_user_question 工具；arguments 严格为：' +
      '{"questions":[{"id":"q1","question":"继续吗？","options":[{"label":"是"},{"label":"否"}]},{"id":"q2","question":"你的名字？"}]}' +
      '。调用后停下来等我回答，不要自己回答。';
    await input.fill(prompt);
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByText('继续吗？', { exact: false })).toBeVisible({ timeout: 120_000 });
    await expect(page.getByText('你的名字？', { exact: false })).toBeVisible();
    await expect(page.getByRole('radio').first()).toBeVisible({ timeout: 120_000 });
    await page.getByRole('radio').first().click();
    const freeText = page.getByPlaceholder('写下你的回答');
    if ((await freeText.count()) > 0) {
      await freeText.fill('测试用户');
    } else {
      await page.getByPlaceholder('自己写').fill('测试用户');
    }
    await page.getByRole('button', { name: '提交回答' }).click();
    await expect(page.getByText('测试用户', { exact: true })).toBeVisible({ timeout: 120_000 });
    await page.screenshot({ path: path.join(evidence, 'dod5-ask-answered.png'), fullPage: true });

    await input.fill(prompt);
    await page.getByRole('button', { name: '发送' }).click();
    await expect(page.getByText('继续吗？', { exact: false })).toBeVisible({ timeout: 120_000 });
    await page.getByRole('button', { name: '不答' }).click();
    await page.screenshot({ path: path.join(evidence, 'dod5-ask-dismissed.png'), fullPage: true });
  } finally {
    await fixture.close();
  }
});
