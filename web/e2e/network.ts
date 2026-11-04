import type { Page } from '@playwright/test';

/** 只让下一次 send 的 POST 失败一次（其余 API 照常），用来做「没送到内核」。 */
export async function failNextSend(page: Page): Promise<void> {
  let failed = false;
  await page.route(/\/api(\?|$)/, async (route) => {
    const request = route.request();
    const body = request.postData() ?? '';
    if (!failed && body.includes('"method":"send"')) {
      failed = true;
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: '{"error":"boom"}',
      });
      return;
    }
    await route.continue();
  });
}
