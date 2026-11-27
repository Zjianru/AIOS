//! Kind 派写任务 → ThinAgent 用 office_create 生成 xlsx → 结果卡/交付记录显示可点开预览 + 未做视觉自检。

import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';

import { expect, test } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskRow(fixture: { dataDir: string }, title: string): { id: string; write_scope?: string } {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    for (const row of rows) {
      const task = JSON.parse(row.value) as { id: string; title: string; write_scope?: string };
      if (task.title === title) return task;
    }
  } finally {
    db.close();
  }
  throw new Error('找不到任务：' + title);
}

test('Office 生成：任务出 xlsx，结果卡和交付记录可点开预览并写「未做视觉自检」', async ({
  page,
  request,
}) => {
  test.setTimeout(240_000);
  const fixture = await startKernelFixture({ t094: 'office' });
  try {
    const enabled = await apiPost(request, fixture, {
      method: 'plugin_set',
      id: 'office',
      enabled: true,
    });
    expect(enabled.ok()).toBeTruthy();
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('给 hello.txt 旁边的团队做一张水果表');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const card = page.locator('[data-kind="task-result"][data-state="review"]').last();
    await expect(card).toBeVisible({ timeout: 60_000 });
    await expect(card.locator('[data-kind="office-deliverable"]')).toBeVisible({ timeout: 30_000 });
    await expect(card.locator('[data-kind="no-visual-check"]')).toContainText('未做视觉自检');

    const task = taskRow(fixture, '给 hello.txt 旁边的团队做一张水果表');
    const generated = path.join(
      fixture.dataDir,
      'library',
      'office',
      task.id,
      '水果表.xlsx',
    );
    const bytes = await import('node:fs/promises').then((fs) => fs.stat(generated));
    expect(bytes.size).toBeGreaterThan(1000);

    await page.goto(urlOf(fixture, '#taskchat:' + task.id));
    const delivery = page.locator('[data-kind="task-delivery"]').first();
    await expect(delivery).toBeVisible({ timeout: 30_000 });
    await delivery.locator('[data-kind="delivery-toggle"]').click();
    await expect(delivery.locator('[data-kind="office-deliverable"]')).toBeVisible({
      timeout: 20_000,
    });
    await expect(delivery.locator('[data-kind="no-visual-check"]')).toContainText('未做视觉自检');
    await delivery.locator('[data-kind="office-deliverable"] button').click();
    await expect(page.locator('[data-kind="office-preview"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="office-no-fidelity"]')).toContainText('没有保真预览');
  } finally {
    await fixture.close();
  }
});
