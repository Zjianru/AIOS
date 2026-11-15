import { test, expect, type Page } from '@playwright/test';
import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

/** T058：交付记录——任务页一行收起、展开五项；程序门未确定；接受后词变；等你入口。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

function taskRow(fixture: { dataDir: string }, title: string): { id: string } {
  const db = new DatabaseSync(path.join(fixture.dataDir, 'tasks.sqlite'), { readOnly: true });
  try {
    const rows = db.prepare('SELECT value FROM task ORDER BY rowid').all() as { value: string }[];
    for (const row of rows) {
      const task = JSON.parse(row.value) as { id: string; title: string };
      if (task.title === title) return task;
    }
  } finally {
    db.close();
  }
  throw new Error('找不到任务：' + title);
}

async function sendKind(page: Page, text: string) {
  await page.getByPlaceholder('和 Kind 说…').fill(text);
  await page.getByRole('button', { name: '发送', exact: true }).click();
}

const delivery = (page: Page) => page.locator('[data-kind="task-delivery"]').first();

test('交付记录：任务页收起一行，展开五项；接受后那一行变成你接受了', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const card = page
      .locator('[data-kind="task-result"][data-state="review"]')
      .last();
    await expect(card).toBeVisible({ timeout: 40_000 });
    const task = taskRow(fixture, '把 hello.txt 的判重改成按 url');
    await page.goto(urlOf(fixture, '#taskchat:' + task.id));
    await expect(delivery(page)).toBeVisible({ timeout: 30_000 });
    await expect(delivery(page).locator('[data-kind="delivery-word"]')).toHaveText('等你接受');
    await expect(delivery(page)).toContainText('第 1 次结果 · 第 1 版要求');
    await delivery(page).locator('[data-kind="delivery-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-what"]')).toContainText(
      'hello.txt 里的判重',
    );
    await expect(delivery(page).locator('[data-kind="delivery-changed"]')).toContainText(
      '改了 1 个文件',
    );
    await expect(delivery(page).locator('[data-kind="delivery-changed"]')).toContainText('hello.txt');
    await expect(delivery(page).locator('[data-kind="delivery-verdict"]')).toContainText(
      '未证明：没安排门任务',
    );
    await expect(delivery(page).locator('[data-kind="delivery-record"]')).toContainText(
      '没有记录：没有程序跑过检查',
    );
    await expect(delivery(page).locator('[data-kind="delivery-missing"]')).toContainText(
      '没有记录到没验证',
    );
    await expect(delivery(page).locator('[data-kind="delivery-where"]')).toContainText(
      '没有外部接续',
    );
    await expect(delivery(page).locator('[data-kind="delivery-accepted"]')).toContainText(
      '这一份还没接受',
    );

    // 回 Kind 接受，再回任务页：同一份记录的词和接受行都变了。
    await page.goto(urlOf(fixture));
    await card.getByRole('button', { name: '接受', exact: true }).click();
    await expect(card.locator('[data-kind="task-accepted"]')).toContainText('你接受了');
    await page.goto(urlOf(fixture, '#taskchat:' + task.id));
    await expect(delivery(page).locator('[data-kind="delivery-word"]')).toHaveText('你接受了', {
      timeout: 30_000,
    });
    await delivery(page).locator('[data-kind="delivery-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-verdict"]')).toContainText(
      '没安排门任务：你接受了这一份',
    );
    await expect(delivery(page).locator('[data-kind="delivery-accepted"]')).toContainText(
      '你接受了 · 第 1 版要求',
    );
  } finally {
    await fixture.close();
  }
});

test('交付记录：程序门通过时判定和逐条检查都在这份记录里', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t057: 'pass' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    // 等程序门跑完（结果卡出现）。
    await expect(
      page
        .locator('[data-kind="task-result"]')
        .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '独立核查' }) })
        .first(),
    ).toBeVisible({ timeout: 90_000 });
    const guarded = taskRow(fixture, '先查行数');
    await page.goto(urlOf(fixture, '#taskchat:' + guarded.id));
    await expect(delivery(page)).toBeVisible({ timeout: 30_000 });
    await expect(delivery(page).locator('[data-kind="delivery-word"]')).toHaveText('等你接受');
    await delivery(page).locator('[data-kind="delivery-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-verdict"]')).toContainText(
      '程序门「独立核查」程序判了通过',
    );
    await expect(delivery(page).locator('[data-kind="delivery-record"]')).toContainText(
      '程序记的：第 1 版命令，2 条里跑了 2 条',
    );
    await delivery(page).locator('[data-kind="delivery-checks-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-check"]')).toHaveCount(2);
    await expect(delivery(page).locator('[data-kind="delivery-check"]').first()).toContainText(
      '退出码 0',
    );
  } finally {
    await fixture.close();
  }
});

test('交付记录：程序门未确定写原因和没跑的命令，那一行是检查未证明', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t057: 'timeout' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '先查 notes.md 有几行，改完找个独立的核查看一眼，我来拍板，过了再写结果');
    const confirm = page.locator('[data-kind="confirm-card"]').first();
    await expect(confirm).toBeVisible({ timeout: 40_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    await expect(
      page
        .locator('[data-kind="task-result"]')
        .filter({ has: page.locator('[data-kind="task-title"]', { hasText: '独立核查' }) })
        .first(),
    ).toBeVisible({ timeout: 90_000 });
    const guarded = taskRow(fixture, '先查行数');
    await page.goto(urlOf(fixture, '#taskchat:' + guarded.id));
    await expect(delivery(page)).toBeVisible({ timeout: 30_000 });
    await expect(delivery(page).locator('[data-kind="delivery-word"]')).toHaveText('检查未证明');
    await delivery(page).locator('[data-kind="delivery-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-verdict"]')).toContainText('未确定');
    await expect(delivery(page).locator('[data-kind="delivery-missing"]')).toContainText(
      '超时：sleep 5',
    );
    await expect(delivery(page).locator('[data-kind="delivery-missing"]')).toContainText(
      '没跑的：true',
    );
  } finally {
    await fixture.close();
  }
});

test('等你：接受项点「看交付记录」直接到任务页', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t030: 'write' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    await expect(
      page.locator('[data-kind="task-result"][data-state="review"]').last(),
    ).toBeVisible({ timeout: 40_000 });
    await page.goto(urlOf(fixture, '#work'));
    const accept = page
      .locator('[data-kind="wait-group"][data-wait-group="accept"]')
      .locator('[data-action="wait-open"]')
      .first();
    await expect(accept).toContainText('看交付记录');
    await accept.click();
    await expect(page).toHaveURL(/#taskchat:/);
    await expect(page.locator('[data-pane="task-conversation"]')).toBeVisible({ timeout: 30_000 });
    await expect(delivery(page)).toBeVisible({ timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('交付记录：第 2 份跟着走，第 1 份折叠进历史，改要求一版一行', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t054: 'update' });
  try {
    await page.goto(urlOf(fixture));
    await sendKind(page, '把 hello.txt 的判重改成按 url');
    const first = page.locator('[data-kind="task-result"][data-state="review"]').last();
    await expect(first).toBeVisible({ timeout: 60_000 });
    await first.locator('[data-kind="task-modify-requirement"]').click();
    const input = page.locator('[data-kind="composer-input"]');
    await input.fill('标题也拿不到的别丢，用域名加路径当键');
    await page.getByRole('button', { name: '发送', exact: true }).click();
    const confirm = page.locator('[data-kind="confirm-card"]').last();
    await expect(confirm).toBeVisible({ timeout: 60_000 });
    await confirm.locator('[data-action="card-option"]').first().click();
    await confirm.locator('[data-action="card-confirm"]').click();
    await expect(confirm).toHaveAttribute('data-state', 'done', { timeout: 40_000 });
    const second = page.locator('[data-kind="task-result"][data-state="review"]').last();
    await expect(second).toContainText('第 2 版要求', { timeout: 60_000 });

    const task = taskRow(fixture, '把 hello.txt 的判重改成按 url');
    await page.goto(urlOf(fixture, '#taskchat:' + task.id));
    await expect(delivery(page).locator('[data-kind="delivery-word"]')).toHaveText('等你接受', {
      timeout: 30_000,
    });
    await expect(delivery(page)).toContainText('第 2 次结果 · 第 2 版要求');
    await delivery(page).locator('[data-kind="delivery-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-accepted"]')).toContainText(
      '第 2 版 · 改了要求',
    );
    await expect(delivery(page).locator('[data-kind="delivery-history-toggle"]')).toHaveText(
      '之前的结果 1 份',
    );
    await delivery(page).locator('[data-kind="delivery-history-toggle"]').click();
    await expect(delivery(page).locator('[data-kind="delivery-history"]')).toContainText(
      '第 1 次结果 · 第 1 版要求',
    );
  } finally {
    await fixture.close();
  }
});

