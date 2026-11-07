import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

const FIXED = '读不出来时沿用上一次读到的；从没读到过就都不发。';
const UNKNOWN = 'settings.json 读不出来';

/// settings.json 从没读到过（启动前就坏了）：serve 照样起，页面不把空表当「没有屏蔽」，
/// 发一句在出站之前被拒，失败块写 settings.json 读不出来并带路径。
test('settings.json 从没读到过：页面说读不出来，发一句的失败块带路径', async ({ page }) => {
  const fixture = await startKernelFixture({ settingsFault: 'corrupt' });
  try {
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const section = page.locator('[data-settings="masked"]');
    await expect(section).toBeVisible({ timeout: 20_000 });

    // 固定说明一直在；动态警告只写「settings.json 读不出来」，不写路径/服务端原因。
    await expect(section).toContainText(FIXED);
    await expect(section.locator('[data-settings="mask-unknown"]')).toBeVisible();
    await expect(section).toContainText(UNKNOWN);
    await expect(section).not.toContainText(fixture.dataDir);
    await expect(section).not.toContainText('EOF while parsing');
    // 不知道的时候不能把空表说成「还没有屏蔽任何模型」。
    await expect(section).not.toContainText('还没有屏蔽任何模型');

    await page.getByRole('button', { name: 'Kind', exact: true }).click();
    await page.getByPlaceholder('和 Kind 说…').fill('坏文件还发吗');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const failure = page.locator('[data-kind="failure"]');
    await expect(failure).toBeVisible({ timeout: 30_000 });
    await expect(failure).toContainText(UNKNOWN);
    await expect(failure).toContainText(fixture.dataDir);
    await expect(failure).toContainText('不知道哪些模型被屏蔽了');
    // 在发往厂商之前就被拒：假回放没收到这一轮。
    expect(await fixture.lastModel()).toBeNull();
  } finally {
    await fixture.close();
  }
});

/// settings.json 坏掉但这个进程读到过：沿用上次读到的规则照常列出；页面不显示
/// 「settings.json 读不出来」的动态警告，也不写路径/服务端原因。
test('settings.json 坏掉但读到过：沿用上次的规则，页面不显示读不出来', async ({ page, request }) => {
  const fixture = await startKernelFixture();
  try {
    // 先让 serve 读到一份规则（这一步同时记下「上次读到的」），再把文件弄坏。
    await apiPost(request, fixture, {
      method: 'disabled-models-set',
      models: ['deepseek/deepseek-flash'],
    });
    await writeFile(path.join(fixture.dataDir, 'settings.json'), '{"lastSelection": ');

    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    const section = page.locator('[data-settings="masked"]');
    await expect(section).toBeVisible({ timeout: 20_000 });

    // 坏文件沿用上次读到的表：规则和命中照常列；固定说明在，动态警告不在。
    await expect(section).toContainText('deepseek/deepseek-flash');
    await expect(section).toContainText('现在匹配 1 个');
    await expect(section).toContainText(FIXED);
    await expect(section).not.toContainText(UNKNOWN);
    await expect(section).not.toContainText(fixture.dataDir);
    await expect(section).not.toContainText('EOF while parsing');
  } finally {
    await fixture.close();
  }
});
