import { test, expect } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

/**
 * T064.6：日历页（约定/周、详情四按钮、运行点）、Kind 小条（到点/变了/第 N 轮报回、diff 展开、看约定）、
 * 等你去看。数据由夹具 calendar.sqlite 与根会话日志驱动，服务端真算周计划。
 */

test('日历约定视图：分组、详情四按钮、暂停/恢复/取消二次确认', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible();
    await expect(page.locator('[data-kind="appt-row"]')).toHaveCount(4);
    await expect(page.locator('[data-kind="calendar-group"]')).toHaveCount(4);

    const morning = page.locator('[data-kind="appt-row"][data-appt="ap_morning"]');
    await morning.click();
    const detail = page.locator('[data-kind="appt-detail"][data-appt="ap_morning"]');
    await expect(detail).toBeVisible();
    await expect(detail.locator('[data-kind="detail-words"]')).toContainText('每个工作日早上 9 点');
    // 运行点颜色/合并：sent/skipped/sent×2 都在约定视图。
    await expect(detail.locator('[data-kind="run-dot"]')).toHaveCount(3);
    await expect(detail.locator('[data-kind="run-dot"][data-result="skipped"]')).toHaveCount(1);

    // 暂停 → 恢复。
    await detail.locator('[data-kind="appt-action"][data-action="pause"]').click();
    await expect(page.locator('[data-kind="appt-detail"] [data-action="resume"]')).toBeVisible();
    await expect(morning).toHaveAttribute('data-state', 'paused');
    await page.locator('[data-kind="appt-detail"] [data-action="resume"]').click();
    await expect(morning).toHaveAttribute('data-state', 'active');

    // 立即跑一次按钮存在（这条是到点约定）；取消要二次确认。
    await expect(page.locator('[data-kind="appt-detail"] [data-action="run-now"]')).toBeVisible();
    await page.locator('[data-kind="appt-detail"] [data-action="cancel"]').click();
    await expect(page.locator('[data-kind="appt-cancel-confirm"]')).toBeVisible();
    await page.locator('[data-kind="appt-cancel-confirm"]').click();
    await expect(morning).toHaveAttribute('data-state', 'cancelled');
  } finally {
    await fixture.close();
  }
});

test('点运行点回到 Kind 那一轮', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await page.locator('[data-kind="appt-row"][data-appt="ap_morning"]').click();
    const detail = page.locator('[data-kind="appt-detail"][data-appt="ap_morning"]');
    await expect(detail).toBeVisible();
    // 夹具里最近一次 sent 的 sent_turn 是 3；点它回 Kind 第 3 轮。
    await detail.locator('[data-kind="run-dot"][data-result="sent"]').last().click();
    await expect(page.locator('[data-turn="turn-3"]')).toBeVisible();
    expect(page.url()).toContain('#kind');
  } finally {
    await fixture.close();
  }
});

test('周视图：计划点/运行点、不画跳过、能翻周', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible();
    await page.locator('[data-kind="calendar-view"][data-view="week"]').click();
    await expect(page.locator('[data-kind="calendar-week"]')).toBeVisible();
    await expect(page.locator('[data-kind="week-day"]')).toHaveCount(7);
    // 工作日 9 点的规则应该算出计划点；夹具里的成功/失败运行画成运行点。
    await expect(page.locator('[data-kind="week-plan"]').first()).toBeVisible();
    await expect(page.locator('[data-kind="week-run"]').first()).toBeVisible();
    // 跳过（没变/没送成之外）不进周视图。
    await expect(page.locator('[data-kind="week-run"][data-result="skipped"]')).toHaveCount(0);
    const firstDay = page.locator('[data-kind="week-day"]').first();
    await expect(firstDay).toBeVisible();
    await page.locator('[data-kind="week-shift"][data-dir="next"]').click();
    await expect(page.locator('[data-kind="week-day"]')).toHaveCount(7);
  } finally {
    await fixture.close();
  }
});

test('Kind 小条：到点/变了/第 N 轮报回、展开 diff、看约定', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind');
    const strips = page.locator('[data-kind="wake-strip"]');
    await expect(strips).toHaveCount(3);
    await expect(strips.filter({ hasText: '到点' })).toHaveCount(1);
    const changed = strips.filter({ hasText: '变了' }).first();
    await expectedChanged(changed);
    // 第 N 轮报回。
    await expect(strips.filter({ hasText: '第 2 轮报回' })).toHaveCount(1);
    // 看约定打开对应那一条。
    await strips.filter({ hasText: '变了' }).first().locator('[data-kind="wake-open"]').click();
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible();
    await expect(page.locator('[data-kind="appt-detail"][data-appt="ap_watch"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

async function expectedChanged(changed: import('@playwright/test').Locator): Promise<void> {
  await changed.locator('[data-kind="wake-diff-toggle"]').click();
  const diff = changed.locator('[data-kind="wake-diff"]');
  await expect(diff).toBeVisible();
  await expect(diff).toContainText('+version 3.2.0');
  await expect(diff).toContainText('-version 3.1.0');
}

test('等你去看：循环约定暂停项打开日历详情', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work');
    const item = page.locator('[data-kind="wait-item"]').filter({ hasText: '连续 3 轮没有进展' }).first();
    await expect(item).toBeVisible();
    await item.locator('[data-action="wait-open"]').click();
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible();
    await expect(page.locator('[data-kind="appt-detail"][data-appt="ap_loop"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('内置做梦行：只有暂停/立即跑一次，暂停就是关开关', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible();
    const dream = page.locator('[data-kind="appt-row"][data-appt="builtin:dream"]');
    await expect(dream).toBeVisible();
    await dream.click();
    const detail = page.locator('[data-kind="appt-detail"][data-appt="builtin:dream"]');
    await expect(detail.locator('[data-action="pause"]')).toBeVisible();
    await expect(detail.locator('[data-action="run-now"]')).toBeVisible();
    await expect(detail.locator('[data-action="cancel"]')).toHaveCount(0);
    await detail.locator('[data-action="pause"]').click();
    await expect(dream).toHaveAttribute('data-state', 'paused');
  } finally {
    await fixture.close();
  }
});
