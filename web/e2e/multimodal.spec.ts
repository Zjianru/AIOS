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

test('输入框拖入图片：出图片卡（缩略图/名字/大小）', async ({ page }) => {
  test.setTimeout(120_000);
  const fixture = await startKernelFixture();
  try {
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
