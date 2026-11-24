import { test, expect } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

const urlOf = (fixture: { baseUrl: string; token: string }) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

interface StoredTask {
  id: string;
  title: string;
}

function tasks(dataDir: string): StoredTask[] {
  const db = new DatabaseSync(path.join(dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    return rows.map((row) => JSON.parse(row.value) as StoredTask);
  } finally {
    db.close();
  }
}

test('旧任务没有原话：任务页不出「你说的」，折叠标题照旧', async ({ page }) => {
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    const task = tasks(fixture.dataDir).find((row) => row.title) as StoredTask;
    await page.goto(urlOf(fixture).replace('#kind', '#taskchat:' + task.id));
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('[data-kind="task-said"]')).toHaveCount(0);
    await expect(page.getByText('任务要求和 DoD')).toBeVisible();
    await page.reload();
    await expect(page.locator('[data-kind="task-said"]')).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
