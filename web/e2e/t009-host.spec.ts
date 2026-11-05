import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }, route: string) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#' + route;

/// T009.5 清单 7：ready 带 host.home，页面显示的日志位置缩写成 ~/…
test('对话丢失的日志路径用 ~/ 缩写', async ({ page }) => {
  const fixture = await startKernelFixture({ rootFault: 'lost', home: 'dataDir' });
  try {
    await page.goto(urlOf(fixture, 'kind'));
    await expect(page.getByText('Kind 的对话找不到了')).toBeVisible({ timeout: 30_000 });
    // 丢失的是会话日志文件：路径在 ~/sessions/… 下；用 dataDir 作 HOME 时显示 ~/…
    await expect(page.getByText(/^~\/sessions\/.+\.jsonl$/)).toBeVisible();
    await expect(page.getByText(fixture.dataDir + '/sessions/')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T009.5 清单 7：设置页的钥匙文件位置也缩写成 ~/…
test('设置页的钥匙文件路径用 ~/ 缩写', async ({ page }) => {
  const fixture = await startKernelFixture({ home: 'dataDir' });
  try {
    await page.goto(urlOf(fixture, 'kind'));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.getByText('~/identity.txt')).toBeVisible({ timeout: 20_000 });
    await expect(page.getByText(fixture.dataDir + '/identity.txt')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});

/// T009.5 清单 8：dockview 的开/关面板播报改成中文。
test('关闭面板的读屏播报是中文', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture, 'kind'));
    await expect(page.getByPlaceholder('和 Kind 说…')).toBeVisible({ timeout: 30_000 });
    // 关掉「对话」面板：关闭动作在标签内（默认悬停才显形，直接派发点击）。
    const tab = page.locator('.dv-tab').filter({ hasText: '对话' }).first();
    await tab.locator('.dv-default-tab-action').dispatchEvent('click');
    // dockview 有两个内部播报区（polite/assertive）；断言其中一条是中文，且没有内置英文。
    await expect(page.locator('.dv-live-region').filter({ hasText: '关闭了 对话' })).toHaveCount(1, {
      timeout: 10_000,
    });
    await expect(page.locator('.dv-live-region').filter({ hasText: 'closed' })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
