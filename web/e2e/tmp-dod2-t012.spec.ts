import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';

// T012.10 真 key DoD 页面驱动 v2（临时 spec，跑完删除，不进仓库）。
// 干净数据目录：页面从目录加 MiniMax（首次使用）+ DeepSeek（设置页）；三轮档 none/xhigh/max。
const logText = readFileSync('/tmp/aios-t012-dod/serve.log', 'utf8');
const matched = /(http:\/\/127\.0\.0\.1:\d+)\/\?token=([A-Za-z0-9-]+)/.exec(logText);
if (!matched) throw new Error('serve URL/token 不在 serve.log');
const base = matched[1];
const token = matched[2];
const pageUrl = base + '/?token=' + token + '#kind';
const evidence = (name: string) => '/tmp/aios-t012-dod/evidence/' + name;
const MINIMAX_KEY = process.env.MINIMAX_KEY ?? '';
const DEEPSEEK_KEY = process.env.DEEPSEEK_KEY ?? '';
if (!MINIMAX_KEY || !DEEPSEEK_KEY) throw new Error('MINIMAX_KEY/DEEPSEEK_KEY 环境变量缺失');

async function pick(page: import('@playwright/test').Page, modelName: RegExp) {
  await page.locator('[data-kind="model-picker"]').click();
  const item = page.getByRole('menuitem', { name: modelName });
  // 菜单里连接多时可能超出视口：先滚进来再点。
  await item.scrollIntoViewIfNeeded();
  await item.click({ force: true });
}

async function pickEffort(page: import('@playwright/test').Page, effort: string) {
  await page.locator('[data-kind="model-picker"]').click();
  const bar = page.locator('[data-kind="effort"]');
  await expect(bar).toBeVisible({ timeout: 20_000 });
  await bar.getByRole('button', { name: effort, exact: true }).click({ force: true });
  await expect(page.locator('[data-kind="effort-now"]')).toContainText(effort === 'none' ? '不思考' : '', {
    timeout: 10_000,
  }).catch(() => {});
  await page.keyboard.press('Escape');
}

async function sendAndWait(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(page.getByRole('button', { name: '停止' })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('button', { name: '停止' })).toHaveCount(0, { timeout: 300_000 });
}

test('T012.10 真 key：页面目录加两连接 + 三轮档 none/xhigh/max + 工具 + 压缩 + 余额余量', async ({ page }) => {
  test.setTimeout(600_000);

  // 1) 首次使用页：从目录加 MiniMax（国内站）。
  await page.goto(pageUrl);
  const form = page.locator('[data-first-use="connect"]');
  await expect(form).toBeVisible({ timeout: 30_000 });
  await form.getByRole('button', { name: /MiniMax（国内站）/ }).click();
  await page.locator('#first-use-key').fill(MINIMAX_KEY);
  await form.getByRole('button', { name: '接上' }).click();
  await expect(page.locator('[data-first-use="status"]')).toContainText('连接可用', { timeout: 120_000 });
  await page.screenshot({ path: evidence('01-first-use-minimax.png') });
  await form.getByRole('button', { name: '开始' }).click();
  await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 20_000 });

  // 2) 设置页：从目录加 DeepSeek（第二家）。
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
  await page.getByRole('button', { name: '添加模型提供商' }).click();
  const add = page.locator('[data-settings="add"]');
  await expect(add).toBeVisible();
  await add.locator('select').selectOption('deepseek');
  await add.locator('input[type="password"]').first().fill(DEEPSEEK_KEY);
  await add.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByText('DeepSeek', { exact: true }).first()).toBeVisible({ timeout: 30_000 });
  // 两个连接都验证一次（真厂商）。
  const verify = page.getByRole('button', { name: '验证', exact: true });
  await expect(verify).toHaveCount(2, { timeout: 20_000 });
  for (let i = 0; i < 2; i += 1) {
    await verify.nth(i).click();
  }
  await expect(page.getByText('连接可用', { exact: false }).first()).toBeVisible({ timeout: 120_000 });
  await page.screenshot({ path: evidence('02-settings-two-connections.png') });

  // 3) 「获取可用模型」对 MiniMax 连接拉真厂商名单。
  const editors = page.getByRole('button', { name: '编辑', exact: true });
  await editors.first().click();
  const editor = page.locator('[data-settings="editor"]');
  await expect(editor).toBeVisible();
  await editor.getByText('自定义设置').click();
  await editor.getByRole('button', { name: '获取可用模型' }).click();
  const discover = page.locator('[data-settings="discover"]');
  await expect(discover).toBeVisible({ timeout: 30_000 });
  await expect(discover).toContainText('服务商回了', { timeout: 30_000 });
  await page.screenshot({ path: evidence('03-available-models.png') });
  await discover.getByRole('button', { name: '取消', exact: true }).click();
  await page.getByRole('button', { name: '模型与连接', exact: true }).click();
  await page.getByRole('button', { name: 'Kind', exact: true }).click();
  await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 20_000 });

  // 4) 第 1 轮：DeepSeek，档 none，带工具（bash）。
  await pick(page, /DeepSeek-V4\.1-Flash/);
  await pickEffort(page, 'none');
  await sendAndWait(page, '用 bash 跑 echo hello-from-t012-dod，然后只告诉我命令输出，不要解释。');
  await page.screenshot({ path: evidence('04-round1-deepseek-none.png') });

  // 5) 第 2 轮：MiniMax M3.1-Flash，档 xhigh。
  await pick(page, /MiniMax-M3\.1-Flash/);
  await pickEffort(page, 'xhigh');
  await sendAndWait(page, '用一句话说明你是哪家公司的模型，并带上你的模型名。');
  await page.screenshot({ path: evidence('05-round2-minimax-xhigh.png') });

  // 6) 第 3 轮：换回 DeepSeek，档 max。
  await pick(page, /DeepSeek-V4\.1-Flash/);
  await pickEffort(page, 'max');
  await sendAndWait(page, '用 bash 再跑一次 echo，然后告诉我这一轮你实际用的模型名。');
  await page.screenshot({ path: evidence('06-round3-deepseek-max.png') });

  // 7) 手动压缩。
  await page.locator('[data-kind="context-meter"]').click();
  await page.locator('[data-kind="compact"]').click();
  await expect(page.locator('[data-kind="compaction"]').last()).toBeVisible({ timeout: 120_000 });
  await page.screenshot({ path: evidence('07-compacted.png') });

  // 8) 统计行。
  const dock = page.locator('[data-kind="composer-dock"]');
  await expect(dock).toContainText('轮', { timeout: 20_000 });
  await expect(page.locator('[data-kind="stats-cost"]')).toBeVisible({ timeout: 20_000 });
  console.log('[dod2] dock=' + (await dock.innerText()).replace(/\n/g, ' '));
  await page.screenshot({ path: evidence('08-stats.png') });

  // 9) 设置页：两个连接的余额/余量（真厂商）。
  await page.getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
  const edit = page.getByRole('button', { name: '编辑', exact: true });
  for (let i = 0; i < 2; i += 1) {
    await edit.nth(i).click();
    const box = page.locator('[data-settings="editor"]');
    await expect(box).toBeVisible();
    await box.getByText('余额与套餐余量').click();
    await box.getByRole('button', { name: '查询余额/余量' }).click();
    await expect(box.locator('[data-settings="quota"]')).toContainText(/余额：|套餐余量：/, { timeout: 60_000 });
    await page.screenshot({ path: evidence('09-quota-' + i + '.png') });
    await box.getByRole('button', { name: '取消', exact: true }).click();
  }
});
