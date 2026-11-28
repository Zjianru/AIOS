import { expect, test } from '@playwright/test';
import { apiPost, startKernelFixture } from './service-fixture';

/** T093.2：设置页输入列、图片附件卡。 */

const urlOf = (fixture: { baseUrl: string; token: string }, hash = '#kind') =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + hash;

test('设置页每个模型有「输入」列：跟目录/文字/文字+图片', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture));
    await page.getByRole('button', { name: '设置', exact: true }).click();
    await expect(page.locator('[data-settings="model"]')).toBeVisible({ timeout: 20_000 });
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const editor = page.locator('[data-settings="editor"]');
    await expect(editor).toBeVisible();
    await editor.getByText('自定义设置').click();
    const input = editor.locator('[data-settings="model-input"]').first();
    await expect(input).toBeVisible();
    await expect(input.locator('option')).toHaveText(['跟目录', '文字', '文字+图片']);
    await input.selectOption('text+image');
    await expect(input).toHaveValue('text+image');
    // 保存后重新打开，覆盖值还在（空=跟目录、填=覆盖，和价格同路）。
    await editor.getByRole('button', { name: '保存', exact: true }).click();
    await expect(editor).toHaveCount(0);
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const reopened = page.locator('[data-settings="editor"]');
    await expect(reopened).toBeVisible();
    await reopened.getByText('自定义设置').click();
    await expect(reopened.locator('[data-settings="model-input"]').first()).toHaveValue('text+image');
    // 「文字」覆盖与清空回「跟目录」也各有落点。
    const reopenedInput = reopened.locator('[data-settings="model-input"]').first();
    await reopenedInput.selectOption('text');
    await reopened.getByRole('button', { name: '保存', exact: true }).click();
    await expect(reopened).toHaveCount(0);
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const textOnly = page.locator('[data-settings="editor"]');
    await textOnly.getByText('自定义设置').click();
    await expect(textOnly.locator('[data-settings="model-input"]').first()).toHaveValue('text');
    await textOnly.locator('[data-settings="model-input"]').first().selectOption('');
    await textOnly.getByRole('button', { name: '保存', exact: true }).click();
    await expect(textOnly).toHaveCount(0);
    await page.getByRole('button', { name: '编辑', exact: true }).first().click();
    const follow = page.locator('[data-settings="editor"]');
    await follow.getByText('自定义设置').click();
    await expect(follow.locator('[data-settings="model-input"]').first()).toHaveValue('');
  } finally {
    await fixture.close();
  }
});

test('输入框拖入图片：出图片卡（缩略图/名字/大小）', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture();
  try {
    // 目录里 deepseek-flash 现在声明了 image；这一条要验「看不了图」的卡，显式把连接
    // 的输入覆盖成只有文字，别让目录升级把用例意图改掉。
    await apiPost(request, fixture, {
      method: 'connection-save',
      id: 'deepseek',
      name: 'DeepSeek',
      model: 'deepseek-flash',
      endpoint: fixture.fakeEndpoint,
      apiKey: 'test-key',
      preset: 'deepseek',
      models: [{ id: 'deepseek-flash', input: ['text'] }],
    });
    await page.goto(urlOf(fixture, '#kind'));
    const composer = page.locator('[data-kind="composer"]');
    await expect(composer).toBeVisible({ timeout: 30_000 });
    const dataTransfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      const bytes = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);
      transfer.items.add(new File([bytes], 'shot.png', { type: 'image/png' }));
      return transfer;
    });
    await composer.dispatchEvent('drop', { dataTransfer });
    const card = page.locator('[data-kind="attachment-image"]');
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card).toContainText('shot.png');
    await expect(card.locator('img')).toHaveAttribute('src', /^data:image\/png;base64,/);
    // 测试夹具的当前模型按目录看不了图：卡上照实写，仍允许发送。
    await expect(card.locator('[data-kind="attachment-image-blind"]')).toContainText('这个模型看不了图');
    // 非文字文件也收：PDF 出类型/大小卡。
    const pdfTransfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([new Uint8Array([37, 80, 68, 70, 45, 49, 46, 55])], 'report.pdf', { type: 'application/pdf' }));
      return transfer;
    });
    await composer.dispatchEvent('drop', { dataTransfer: pdfTransfer });
    const svgTransfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File(["<svg xmlns='http://www.w3.org/2000/svg'/>"], 'pic.svg', { type: 'image/svg+xml' }));
      return transfer;
    });
    await composer.dispatchEvent('drop', { dataTransfer: svgTransfer });
    const svgCard = page.locator('[data-kind="file-card"]');
    await expect(svgCard).toBeVisible({ timeout: 20_000 });
    await expect(svgCard).toContainText('pic.svg');
    await expect(page.locator('[data-kind="attachment-image"]')).toHaveCount(1, { timeout: 5_000 });
    const binary = page.locator('[data-kind="attachment-binary"]');
    await expect(binary).toBeVisible({ timeout: 20_000 });
    await expect(binary).toContainText('report.pdf');
    await expect(binary).toContainText('PDF');
    // 选文件按钮走 hidden input，同样出非文字卡。
    await page.setInputFiles('[data-kind="composer-file-input"]', {
      name: 'picked.zip',
      mimeType: 'application/zip',
      buffer: Buffer.from([80, 75, 3, 4]),
    });
    await expect(page.locator('[data-kind="attachment-binary"]')).toHaveCount(2);
    await expect(page.locator('[data-kind="attachment-binary"]').last()).toContainText('picked.zip');
  } finally {
    await fixture.close();
  }
});


test('消息图片真实路径：附一张图→发送→模型回话→消息里有图，刷新后还在', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture, '#kind'));
    const composer = page.locator('[data-kind="composer"]');
    await expect(composer).toBeVisible({ timeout: 30_000 });
    const dataTransfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], 'sent.png', { type: 'image/png' }));
      return transfer;
    });
    await composer.dispatchEvent('drop', { dataTransfer });
    await expect(page.locator('[data-kind="attachment-image"]')).toBeVisible({ timeout: 20_000 });
    await page.locator('[data-kind="send"]').click();
    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row.locator('[data-kind="attachment-image"]')).toBeVisible({ timeout: 20_000 });
    await expect(row.locator('[data-kind="attachment-image"]')).toContainText('sent.png');
    // e2e 的模拟模型回话（默认夹具回放）走完这一轮。
    await expect(page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last()).toBeVisible({ timeout: 90_000 });
    await page.reload();
    const history = page.locator('[data-kind="user-message-row"]').last();
    await expect(history.locator('[data-kind="attachment-image"]')).toContainText('sent.png', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('任务对话：任务冻结模型看不了图时，图片卡带提示（seesImages）', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    const response = await apiPost(request, fixture, { method: 'work' });
    const work = (await response.json()) as {
      tasks: { id: string; title: string }[];
    };
    const task = work.tasks.find((item) => item.title === '查 A');
    expect(task, '夹具里要有任务「查 A」').toBeTruthy();
    // 目录里 deepseek-flash 现在标了 text+image；这一条只验证「页面拿 seesImages 画盲提示」：
    // 先用真实请求拿一份 task-read 响应，把该字段覆盖成 false，再交给页面（真实服务形状）。
    const readResponse = await apiPost(request, fixture, { method: 'task-read', taskId: task!.id });
    const readJson = (await readResponse.json()) as { task?: Record<string, unknown> };
    if (readJson.task) readJson.task.seesImages = false;
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method !== 'task-read') {
        await route.continue();
        return;
      }
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(readJson) });
    });
    await page.goto(urlOf(fixture, '#taskchat:' + task!.id));
    const composer = page.locator('[data-kind="composer"]');
    await expect(composer).toBeVisible({ timeout: 30_000 });
    const dataTransfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], 'task.png', { type: 'image/png' }));
      return transfer;
    });
    await composer.dispatchEvent('drop', { dataTransfer });
    const card = page.locator('[data-kind="attachment-image"]');
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card.locator('[data-kind="attachment-image-blind"]')).toContainText('这个模型看不了图');
  } finally {
    await fixture.close();
  }
});


test('目录读不到时按看不了图：peek 失败也不猜成能看', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture();
  try {
    await page.route('**/api', async (route) => {
      let method = '';
      try {
        method = (route.request().postDataJSON() as { method?: string }).method ?? '';
      } catch {
        // 非 JSON 请求原样放行。
      }
      if (method === 'available-models') {
        await route.fulfill({ status: 500, contentType: 'application/json', body: '{}' });
        return;
      }
      await route.continue();
    });
    await page.goto(urlOf(fixture, '#kind'));
    const composer = page.locator('[data-kind="composer"]');
    await expect(composer).toBeVisible({ timeout: 30_000 });
    const dataTransfer = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], 'failed-model.png', { type: 'image/png' }));
      return transfer;
    });
    await composer.dispatchEvent('drop', { dataTransfer });
    const card = page.locator('[data-kind="attachment-image"]');
    await expect(card).toBeVisible({ timeout: 20_000 });
    await expect(card.locator('[data-kind="attachment-image-blind"]')).toContainText('这个模型看不了图');
  } finally {
    await fixture.close();
  }
});

test('@ 对象：任务/报告候选分组，选中出引用片，发送后消息里在，刷新还在', async ({ page, request }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    await page.goto(urlOf(fixture, '#kind'));
    const box = page.locator('[data-kind="composer-input"]');
    await expect(box).toBeVisible({ timeout: 30_000 });
    await box.fill('@');
    const menu = page.locator('[data-kind="at-menu"]');
    await expect(menu).toBeVisible({ timeout: 10_000 });
    await expect(menu.locator('[data-group="task"]').first()).toBeVisible();
    await expect(menu.locator('[data-kind="mention-group"]').filter({ hasText: '任务' })).toHaveCount(1);
    await menu.locator('[data-group="task"] [data-kind="mention-item"]').first().click();
    const objects = page.locator('[data-kind="attachment-object"]');
    await expect(objects).toHaveCount(1);
    await expect(objects.first()).toHaveAttribute('data-object-type', 'task');
    // 再选一个报告。
    await box.fill('@');
    await expect(menu).toBeVisible({ timeout: 10_000 });
    await expect(menu.locator('[data-kind="mention-group"]').filter({ hasText: '报告' })).toHaveCount(1);
    await menu.locator('[data-group="report"] [data-kind="mention-item"]').first().click();
    await expect(objects).toHaveCount(2);
    await page.locator('[data-kind="send"]').click();

    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row.locator('[data-kind="attachment-object"]')).toHaveCount(2, { timeout: 20_000 });
    await expect(row.locator('[data-object-type="report"]')).toHaveCount(1);
    await expect(page.locator('[data-kind="assistant-answer"], [data-kind="answer"]').last()).toBeVisible({ timeout: 90_000 });
    await page.reload();
    const history = page.locator('[data-kind="user-message-row"]').last();
    await expect(history.locator('[data-kind="attachment-object"]')).toHaveCount(2, { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

async function selectTextIn(
  page: import('@playwright/test').Page,
  locator: import('@playwright/test').Locator,
  needle: string,
) {
  await locator.scrollIntoViewIfNeeded();
  await locator.evaluate((element, needleText) => {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node && !(node.textContent ?? '').includes(needleText)) node = walker.nextNode();
    if (!node) throw new Error('选区文字没找到：' + needleText);
    const start = (node.textContent ?? '').indexOf(needleText);
    const range = document.createRange();
    range.setStart(node, start);
    range.setEnd(node, start + needleText.length);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    const target = range.commonAncestorContainer.parentElement ?? element;
    target.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  }, needle);
}

test('报告卡选中文字：引用到 Kind 输入框，发送成引用块，刷新还在', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ taskResult: 'ready' });
  try {
    await page.goto(urlOf(fixture, '#kind'));
    await page.getByPlaceholder('和 Kind 说…').fill('帮我查一下新闻归档里的判重规则');
    await page.locator('[data-kind="send"]').click();
    const card = page.locator('[data-kind="task-result"]').last();
    await expect(card).toBeVisible({ timeout: 30_000 });
    await expect(card).toHaveAttribute('data-state', 'ready', { timeout: 30_000 });
    const summary = '判重用的是原始链接';
    await selectTextIn(page, card, summary);
    const quoteButton = page.locator('[data-kind="quote-selection"]');
    await expect(quoteButton).toBeVisible();
    await quoteButton.click();
    const quoteCard = page.locator('[data-kind="composer-cards"] [data-kind="quote-card"]').last();
    await expect(quoteCard).toBeVisible();
    await expect(quoteCard.locator('[data-kind="quote-source"]')).toContainText('报告');
    await page.locator('[data-kind="send"]').click();
    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row.locator('[data-kind="quote-card"]')).toBeVisible({ timeout: 20_000 });
    await expect(row.locator('[data-kind="quote-source"]')).toContainText('报告');
    await page.reload();
    const history = page.locator('[data-kind="user-message-row"]').last();
    await expect(history.locator('[data-kind="quote-card"]')).toBeVisible({ timeout: 30_000 });
    await expect(history.locator('[data-kind="quote-source"]')).toContainText('报告');
  } finally {
    await fixture.close();
  }
});






const readPendingQuote = (text = '评测记录') => ({
  text,
  source: { kind: 'file', id: '评测记录.md', title: '评测记录.md', version: 1 },
});

test('阅读页引用（Kind 侧）：pending quote → 引用卡 → 发送成引用块，刷新还在', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture();
  try {
    // 真实跨页路径：先在另一个页面写入 pending quote，再导航到 Kind；
    // 不在 Kind 页设置后 reload，避免 Composer 已挂载时消费时序不确定。
    await page.goto(urlOf(fixture, '#now'));
    await page.evaluate((quote) => {
      localStorage.setItem('aios.kind.pending-quote', JSON.stringify(quote));
    }, readPendingQuote());
    await page.goto(urlOf(fixture, '#kind'));
    await expect(page.locator('[data-kind="composer-input"]')).toBeVisible({ timeout: 30_000 });
    const quoteCard = page.locator('[data-kind="composer-cards"] [data-kind="quote-card"]').last();
    await expect(quoteCard).toBeVisible({ timeout: 20_000 });
    await expect(quoteCard.locator('[data-kind="quote-source"]')).toContainText('阅读页 · 评测记录.md · 版本 1');
    await page.locator('[data-kind="send"]').click();
    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row.locator('[data-kind="quote-card"]')).toBeVisible({ timeout: 20_000 });
    await expect(row.locator('[data-kind="quote-source"]')).toContainText('阅读页');
    await page.reload();
    await expect(page.locator('[data-kind="user-message-row"]').last().locator('[data-kind="quote-source"]')).toContainText('阅读页', { timeout: 30_000 });
  } finally {
    await fixture.close();
  }
});

test('Kind 已有草稿时，阅读页 pending quote 也进输入框、草稿不丢', async ({ page }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture();
  try {
    await page.goto(urlOf(fixture, '#kind'));
    const box = page.locator('[data-kind="composer-input"]');
    await expect(box).toBeVisible({ timeout: 30_000 });
    await box.fill('已有草稿一句');
    await page.waitForTimeout(300);
    await page.evaluate((quote) => {
      localStorage.setItem('aios.kind.pending-quote', JSON.stringify(quote));
    }, readPendingQuote());
    await page.reload();
    await expect(page.locator('[data-kind="composer-input"]')).toHaveValue('已有草稿一句', { timeout: 20_000 });
    const quoteCard = page.locator('[data-kind="composer-cards"] [data-kind="quote-card"]').last();
    await expect(quoteCard).toBeVisible({ timeout: 20_000 });
    await expect(quoteCard.locator('[data-kind="quote-source"]')).toContainText('阅读页');
    await page.locator('[data-kind="send"]').click();
    const row = page.locator('[data-kind="user-message-row"]').last();
    await expect(row.locator('[data-kind="quote-card"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="user-bubble"]').last()).toContainText('已有草稿一句');
  } finally {
    await fixture.close();
  }
});

test('任务对话：选中引用 + @ 对象一起发送，消息行上两样都在', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture({ t062TaskGraphs: true });
  try {
    const response = await apiPost(request, fixture, { method: 'work' });
    const work = (await response.json()) as { tasks: { id: string; title: string }[] };
    let taskId = '';
    for (const candidate of work.tasks) {
      const read = await apiPost(request, fixture, { method: 'task-read', taskId: candidate.id });
      const body = (await read.json()) as { task?: { running?: boolean; backgroundRunning?: boolean; cancelledAt?: number | null } };
      if (!body.task?.running && !body.task?.backgroundRunning && !body.task?.cancelledAt) {
        taskId = candidate.id;
        break;
      }
    }
    expect(taskId, '夹具里要有空闲任务').not.toBe('');
    await page.goto(urlOf(fixture, '#taskchat:' + taskId));
    const box = page.locator('[data-kind="composer-input"]');
    await expect(box).toBeVisible({ timeout: 30_000 });
    // 先发一句，排队消息里有可选文字。
    await box.fill('先记一句');
    await box.press('Enter');
    const firstQueued = page.locator('[data-kind="task-queued"]').last();
    await expect(firstQueued).toContainText('先记一句', { timeout: 20_000 });
    await selectTextIn(page, firstQueued, '先记一句');
    await page.locator('[data-kind="quote-selection"]').click();
    const quoteCard = page.locator('[data-kind="composer-cards"] [data-kind="quote-card"]').last();
    await expect(quoteCard).toBeVisible({ timeout: 20_000 });
    await expect(quoteCard.locator('[data-kind="quote-source"]')).toContainText('任务对话');
    // 再 @ 一个任务对象。
    await page.locator('[data-kind="composer-input"]').fill('@');
    const menu = page.locator('[data-kind="at-menu"]');
    await expect(menu).toBeVisible({ timeout: 10_000 });
    await menu.locator('[data-group="task"] [data-kind="mention-item"]').first().click();
    await expect(page.locator('[data-kind="composer-cards"] [data-kind="attachment-object"]')).toHaveCount(1);
    await box.click();
    await box.press('Enter');
    const rows = page.locator('[data-kind="task-queued"]');
    await expect(rows).toHaveCount(2, { timeout: 20_000 });
    const last = rows.last();
    await expect(last.locator('[data-kind="quote-card"]')).toBeVisible();
    await expect(last.locator('[data-kind="attachment-object"]')).toBeVisible();
  } finally {
    await fixture.close();
  }
});
