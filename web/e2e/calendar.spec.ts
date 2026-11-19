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
    await expect(page.locator('[data-kind="appt-row"]')).toHaveCount(8);
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

test('来源轮可点回 Kind', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await page.locator('[data-kind="appt-row"][data-appt="ap_morning"]').click();
    const detail = page.locator('[data-kind="appt-detail"][data-appt="ap_morning"]');
    await expect(detail.locator('[data-kind="detail-source-turn"]')).toContainText('第 3 轮');
    await detail.locator('[data-kind="detail-source-turn"]').click();
    await expect(page.locator('[data-turn="turn-3"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('循环行显示第几轮与任务状态词', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true, t062TaskGraphs: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    const loop = page.locator('[data-kind="appt-row"][data-appt="ap_loop"]');
    await expect(loop.locator('[data-kind="loop-band"]')).toContainText('第 2 轮');
    await expect(loop.locator('[data-kind="loop-band"]')).toContainText(
      /待补足条件|待执行|执行中|待检查|任务要求已满足|已关闭/,
    );
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

test('T066 故事1-2：Kind 自办块已完成、办完了回轮', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await page.locator('[data-kind="calendar-view"][data-view="week"]').click();
    await expect(page.locator('[data-kind="calendar-week"]')).toBeVisible();
    const block = page.locator('[data-kind="week-run"][data-appt="ap_morning"][data-status="已关闭"]').first();
    await expect(block).toBeVisible();
    await expect(block).toContainText('早报');
    await block.click();
    const detail = page.locator('[data-kind="appt-detail"][data-appt="ap_morning"]');
    await expect(detail).toBeVisible();
    const done = detail.locator('[data-kind="run-kind-done"]').first();
    await expect(done).toContainText('Kind 办完了');
    await done.click();
    await expect(page.locator('[data-turn="turn-3"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('T066 故事3：派出的块跟任务状态、点开任务页', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await page.locator('[data-kind="calendar-view"][data-view="week"]').click();
    const dispatched = page.locator('[data-kind="week-run"][data-task="t066-dispatched-1"]').first();
    await expect(dispatched).toBeVisible();
    await expect(dispatched).toHaveAttribute('data-status', '待执行');
    await dispatched.click();
    await expect(page.locator('[data-kind="task-full-title"]')).toContainText('调查 elkjs 调查');
  } finally {
    await fixture.close();
  }
});

test('T066 一次性结束的块仍在、计划只出在用约定', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    const row = page.locator('[data-kind="appt-row"][data-appt="ap_once"]');
    await expect(row).toHaveAttribute('data-state', 'ended');
    const cancelled = page.locator('[data-kind="appt-row"][data-appt="ap_cancelled"]');
    await expect(cancelled).toHaveAttribute('data-state', 'cancelled');
    await page.locator('[data-kind="calendar-view"][data-view="week"]').click();
    await expect(page.locator('[data-kind="calendar-week"]')).toBeVisible();
    const block = page.locator('[data-kind="week-run"][data-appt="ap_once"]').first();
    await expect(block).toBeVisible();
    await expect(block).toContainText('交周报');
    await expect(block).toHaveAttribute('data-status', '已关闭');
    await expect(page.locator('[data-kind="week-plan"][data-appt="ap_once"]')).toHaveCount(0);
    const cancelledBlock = page.locator('[data-kind="week-run"][data-appt="ap_cancelled"]').first();
    await expect(cancelledBlock).toBeVisible();
    await expect(cancelledBlock).toHaveAttribute('data-status', '已关闭');
  } finally {
    await fixture.close();
  }
});

test('T066 重叠并排：两个块时间撞了分道', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await page.locator('[data-kind="calendar-view"][data-view="week"]').click();
    const overlap = page.locator('[data-kind="week-run"][data-appt="ap_overlap"]').first();
    await expect(overlap).toBeVisible();
    const start = await overlap.getAttribute('data-start');
    const morning = page.locator('[data-kind="week-run"][data-appt="ap_morning"][data-start="' + start + '"]').first();
    await expect(morning).toBeVisible();
    await expect(morning).toHaveAttribute('data-lanes', '2');
    await expect(overlap).toHaveAttribute('data-lanes', '2');
    const morningLane = await morning.getAttribute('data-lane');
    const overlapLane = await overlap.getAttribute('data-lane');
    expect(morningLane).not.toEqual(overlapLane);
  } finally {
    await fixture.close();
  }
});

test('T066 周/月看法：三种切法、红线/今天、斜纹计划、月跳周', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await expect(page.locator('[data-kind="calendar-view"]')).toHaveCount(3);
    await page.locator('[data-kind="calendar-view"][data-view="week"]').click();
    await expect(page.locator('[data-kind="week-day"]')).toHaveCount(7);
    await expect(page.locator('[data-kind="week-plan"]').first()).toBeVisible();
    await expect(page.locator('[data-kind="week-run"]').first()).toBeVisible();
    await expect(page.locator('[data-kind="now-line"]')).toBeVisible();
    await expect(page.locator('[data-kind="dense-strip"]').first()).toBeVisible();
    await expect(page.locator('[data-kind="week-day"][data-today="true"]')).toHaveCount(1);
    // 悬停一块，同一条规则的其他块一起亮。
    const hovered = page.locator('[data-kind="week-run"][data-appt="ap_morning"]').first();
    await hovered.hover();
    await expect(page.locator('[data-kind="week-run"][data-appt="ap_morning"]').last()).toHaveClass(/ring-2/);
    // 月：6x7 格、块、还有 N 条跳周。
    await page.locator('[data-kind="calendar-view"][data-view="month"]').click();
    await expect(page.locator('[data-kind="month-day"]')).toHaveCount(42);
    await expect(page.locator('[data-kind="month-block"]').first()).toBeVisible();
    const more = page.locator('[data-kind="month-more"]').first();
    await expect(more).toBeVisible();
    await more.click();
    await expect(page.locator('[data-kind="calendar-week"]')).toBeVisible();
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

test('内置做梦行：间隔/运行记录、实际点立即跑一次，暂停就是关开关', async ({ page }) => {
  const fixture = await startKernelFixture({ t064Calendar: true });
  try {
    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#work?wv=cal');
    await expect(page.locator('[data-kind="calendar-page"]')).toBeVisible();
    const dream = page.locator('[data-kind="appt-row"][data-appt="builtin:dream"]');
    await expect(dream).toBeVisible();
    await dream.click();
    const detail = page.locator('[data-kind="appt-detail"][data-appt="builtin:dream"]');
    // 内置行显示间隔（rule）和下次触发。
    await expect(detail).toContainText('每 24 小时');
    await expect(detail).toContainText('（Asia/Shanghai）');
    await expect(detail).toContainText('下次触发');
    await expect(detail.locator('[data-action="pause"]')).toBeVisible();
    await expect(detail.locator('[data-action="run-now"]')).toBeVisible();
    await expect(detail.locator('[data-action="cancel"]')).toHaveCount(0);

    // 打开观察许可，再实际点「立即跑一次」：dream.request_now 会写一条 DreamRun，
    // 日历读到的内置运行记录/下次触发随后刷新出来。
    await page.evaluate(async () => {
      await fetch('/api', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ method: 'memory-action', action: { kind: 'set-permit', allow: true } }),
      });
    });
    await detail.locator('[data-action="run-now"]').click();
    await expect(detail.locator('[data-kind="run-dot"]').first()).toBeVisible({ timeout: 20_000 });
    await expect(detail).toContainText('下次触发');
    await expect(detail).toContainText('每 24 小时');

    // 暂停 = 关记忆设置里的做梦开关（页面状态变已暂停）。
    await expect(detail.locator('[data-action="pause"]')).toBeVisible();
    await detail.locator('[data-action="pause"]').click();
    await expect(dream).toHaveAttribute('data-state', 'paused');
  } finally {
    await fixture.close();
  }
});


