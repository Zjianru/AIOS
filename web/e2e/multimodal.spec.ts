import { expect, test } from '@playwright/test';
import { startKernelFixture } from './service-fixture';

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

