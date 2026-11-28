import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#settings:rules';

test('T103.3 规则：新建/版本/冲突/删除持久化', async ({ page }) => {
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    const section = page.locator('[data-settings="rules"]');
    await expect(section).toBeVisible({ timeout: 30_000 });
    const make = async (id: string, scope: string, strictness: string) => {
      await section.locator('[data-rule-id-input]').fill(id);
      await section.locator('[data-rule-scope]').selectOption(scope);
      await section.locator('[data-rule-strictness]').selectOption(strictness);
      await section.locator('[data-rule-category-input]').fill('动作');
      await section.locator('[data-rule-text]').fill('删除文件要问我');
      await section.locator('[data-rule-save]').click();
      await expect(section.locator('[data-rule-id="' + id + '"]')).toBeVisible();
    };
    await make('g-deny', 'global', 'deny');
    await make('p-allow', 'project:p1', 'allow');
    await expect(section.locator('[data-rules-conflicts]')).toContainText('p-allow');
    await expect(section).toContainText('当前版本 v2');
    await page.reload();
    await expect(page.locator('[data-settings="rules"]')).toContainText('当前版本 v2');
    await page.locator('[data-rule-delete="p-allow"]').click();
    await expect(page.locator('[data-rule-id="p-allow"]')).toHaveCount(0);
    await expect(page.locator('[data-settings="rules"]')).toContainText('当前版本 v3');
  } finally {
    await fixture.close();
  }
});
