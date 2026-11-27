//! Office 文件卡与预览（插件关提示、打开后无保真卡、坏文件、Finder/原件按钮）。

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { expect, test } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

const FIXTURE_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures', 'office');

function mimeOf(name: string): string {
  if (name.endsWith('.xlsx')) {
    return 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';
  }
  if (name.endsWith('.docx')) {
    return 'application/vnd.openxmlformats-officedocument.wordprocessingml.document';
  }
  return 'application/vnd.openxmlformats-officedocument.presentationml.presentation';
}

async function sendFile(
  request: any,
  fixture: KernelFixture,
  fileName: string,
  content = '看看这个文件',
) {
  const data = readFileSync(path.join(FIXTURE_DIR, fileName)).toString('base64');
  const put = await apiPost(request, fixture, {
    method: 'attachment-put',
    name: fileName,
    data,
  });
  const body = (await put.json()) as { id?: string; bytes?: number };
  expect(body.id).toBeTruthy();
  const response = await apiPost(request, fixture, {
    method: 'send',
    requestId: 'office-preview-' + fileName + '-' + String(Date.now()) + '-' + String(Math.random()).slice(2),
    content,
    delivery: 'follow-up',
    attachments: [
      { id: body.id, name: fileName, bytes: body.bytes ?? 0, mime: mimeOf(fileName) },
    ],
  });
  expect(response.ok()).toBeTruthy();
  return body.id as string;
}

async function setOfficePlugin(request: any, fixture: KernelFixture, enabled: boolean) {
  const response = await apiPost(request, fixture, {
    method: 'plugin_set',
    id: 'office',
    enabled,
  });
  expect(response.ok()).toBeTruthy();
}

test('文件卡：插件关写「Office 插件没开」，去设置打开后点开是无保真卡', async ({
  page,
  request,
}) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture();
  try {
    await sendFile(request, fixture, 'sample.xlsx');
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    const card = page.locator('[data-kind="attachment-binary"]', { hasText: 'sample.xlsx' }).last();
    await expect(card).toContainText('XLSX · Office 插件没开', { timeout: 30_000 });
    await card.locator('[data-kind="file-open"]').click();
    await expect(page.locator('[data-kind="office-off"]')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="office-off-hint"]')).toContainText('Office 插件没开');
    await expect(page.locator('[data-kind="office-go-plugins"]')).toBeVisible();
    await page.locator('[data-kind="office-go-plugins"]').click();

    await expect(page.locator('[data-kind="plugin-card"]')).toBeVisible({ timeout: 20_000 });
    await expect(page.locator('[data-kind="plugin-state"]')).toHaveText('已关闭');
    await page.locator('[data-kind="plugin-toggle"]').click();
    await expect(page.locator('[data-kind="plugin-state"]')).toHaveText('已开启', {
      timeout: 20_000,
    });

    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);
    const openedCard = page
      .locator('[data-kind="attachment-binary"]', { hasText: 'sample.xlsx' })
      .last();
    await expect(openedCard).toContainText('XLSX', { timeout: 30_000 });
    await expect(openedCard).not.toContainText('Office 插件没开');
    await openedCard.locator('[data-kind="file-open"]').click();
    await expect(page.locator('[data-kind="office-preview"]')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="office-preview"]')).toContainText('只读副本');
    await expect(page.locator('[data-kind="office-no-fidelity"]')).toContainText('没有保真预览');
    await expect(page.locator('[data-kind="office-no-fidelity"]')).toContainText('xlsx');
    await expect(page.getByRole('button', { name: '打开原件' })).toBeVisible();
    await expect(page.getByRole('button', { name: '在 Finder 中显示' })).toBeVisible();
  } finally {
    await fixture.close();
  }
});

test('docx/pptx 同一张无保真卡；坏文件写原因，不白屏', async ({ page, request }) => {
  test.setTimeout(180_000);
  const fixture = await startKernelFixture();
  try {
    await setOfficePlugin(request, fixture, true);
    await sendFile(request, fixture, 'sample.docx');
    await sendFile(request, fixture, 'sample.pptx');
    await sendFile(request, fixture, 'broken.docx');
    await sendFile(request, fixture, 'corrupt.xlsx');
    await page.goto(`${fixture.baseUrl}/?token=${encodeURIComponent(fixture.token)}#kind`);

    const docxCard = page.locator('[data-kind="attachment-binary"]', { hasText: 'sample.docx' }).last();
    await expect(docxCard).toContainText('DOCX', { timeout: 30_000 });
    await docxCard.locator('[data-kind="file-open"]').click();
    await expect(page.locator('[data-kind="office-no-fidelity"]')).toContainText('Word 文档');
    await expect(page.locator('[data-kind="office-text"]')).toContainText('读到的文字（不含版式）');
    await expect(page.locator('[data-kind="office-text"]')).toContainText('第一段');
    await expect(page.locator('[data-kind="office-text"]')).toContainText('格一');
    await page.getByRole('button', { name: '关闭预览' }).click();

    const pptxCard = page.locator('[data-kind="attachment-binary"]', { hasText: 'sample.pptx' }).last();
    await pptxCard.locator('[data-kind="file-open"]').click();
    await expect(page.locator('[data-kind="office-no-fidelity"]')).toContainText('幻灯片');
    await expect(page.locator('[data-kind="office-text"]')).toContainText('第一页标题');
    await page.getByRole('button', { name: '关闭预览' }).click();

    const brokenCard = page.locator('[data-kind="attachment-binary"]', { hasText: 'broken.docx' }).last();
    await brokenCard.locator('[data-kind="file-open"]').click();
    await expect(page.locator('[data-kind="office-preview"]')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('[data-kind="office-broken"]')).toContainText('文件不是有效的 Office 压缩包');
    await expect(page.getByRole('button', { name: '打开原件' })).toBeVisible();
    await page.getByRole('button', { name: '关闭预览' }).click();

    const corruptCard = page.locator('[data-kind="attachment-binary"]', { hasText: 'corrupt.xlsx' }).last();
    await corruptCard.locator('[data-kind="file-open"]').click();
    await expect(page.locator('[data-kind="office-broken"]')).toContainText('文件不是有效的 Office 压缩包');
    await expect(page.getByRole('button', { name: '在 Finder 中显示' })).toBeVisible();
  } finally {
    await fixture.close();
  }
});
