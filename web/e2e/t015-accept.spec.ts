import { expect, test, type APIRequestContext } from '@playwright/test';
import { apiPost, startKernelFixture, type KernelFixture } from './service-fixture';

/**
 * T015 用户验收补充（2026-11-07）：
 * 1. 「回到最新」按钮去掉 motion 位移/淡入，平时静态灰、悬浮变实，滚动时不再抖；
 * 2. 回答正文里的裸文本绝对路径（不限行内 code / 加粗）也渲染成 /local-file 链接，代码块和 URL 不动。
 */
const urlOf = (fixture: KernelFixture) =>
  fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token) + '#kind';

async function finishedTurns(request: APIRequestContext, fixture: KernelFixture): Promise<number> {
  const response = await apiPost(request, fixture, { method: 'follow', lastSeq: 0 });
  expect(response.ok()).toBeTruthy();
  const body = (await response.json()) as { events: { type: string }[] };
  return body.events.filter((event) => event.type === 'turn/end').length;
}

test('回到最新（验收补 1）：滚上去才出现、平时静态灰、悬浮变实、没有位移/淡入动画', async ({ request, page }) => {
  const fixture = await startKernelFixture({});
  try {
    for (let index = 1; index <= 8; index += 1) {
      const response = await apiPost(request, fixture, {
        method: 'send',
        requestId: 't015-accept-scroll-' + String(index),
        content: '滚一滚 ' + String(index),
      });
      expect(response.ok()).toBeTruthy();
      await expect
        .poll(() => finishedTurns(request, fixture), { timeout: 30_000 })
        .toBeGreaterThanOrEqual(index);
    }

    await page.goto(urlOf(fixture));
    await expect(page.getByText('滚一滚 8', { exact: false })).toBeVisible({ timeout: 15_000 });
    const button = page.locator('[data-kind="scroll-latest"]');
    // 在底部时按钮不存在（条件渲染，不是动画藏起来）。
    await expect(button).toHaveCount(0);

    await page.locator('[data-pane="conversation"]').hover();
    await page.mouse.wheel(0, -3000);
    await expect(button).toBeVisible({ timeout: 10_000 });

    // 默认是 --ink-3 的灰，不是正文色 --ink。
    await expect(button).toHaveCSS('color', 'rgb(110, 110, 122)');
    // 悬浮才变实（--ink）。
    await button.hover();
    await expect(button).toHaveCSS('color', 'rgb(237, 237, 242)');

    // 没有 motion 的 translateY / 淡入：transform 的 y 分量为 0，也没有 CSS 动画/过渡。
    const style = await button.evaluate((element) => {
      const computed = getComputedStyle(element);
      return {
        transform: computed.transform,
        translate: computed.translate,
        animationName: computed.animationName,
        transitionDuration: computed.transitionDuration,
      };
    });
    const matrix = /matrix\(([^)]+)\)/.exec(style.transform);
    if (matrix) {
      const parts = matrix[1].split(',').map((value) => Number(value.trim()));
      expect(parts[5]).toBe(0); // matrix 的 translateY
    }
    const translate = style.translate.trim().split(/\s+/);
    expect(translate[1] ?? '0px').toMatch(/^0(px)?$/); // translate: x y
    expect(style.animationName).toBe('none');
    expect(style.transitionDuration).toBe('0s');
  } finally {
    await fixture.close();
  }
});

test('本地文件链接（验收补 B2）：裸文本路径和 open 块都变链接，代码块/URL 不动', async ({ page }) => {
  const fixture = await startKernelFixture({ localFilePath: true });
  try {
    await page.goto(urlOf(fixture));
    await page.getByPlaceholder('和 Kind 说…').fill('文件呢？');
    await page.getByRole('button', { name: '发送', exact: true }).click();

    const answer = page.locator('[data-kind="assistant-answer"]').last();
    await expect(answer).toContainText('已经存好了', { timeout: 15_000 });
    const path = '/private/tmp/aios-t015/存钱计划 30 年.html';
    const href = '/local-file?path=' + encodeURIComponent(path);

    // 正文裸文本：真 <a data-kind="local-file-link" target="_blank">，链接文字就是完整路径。
    const bare = answer.locator('a[data-kind="local-file-link"]');
    await expect(bare).toHaveCount(1);
    await expect(bare).toHaveAttribute('href', href);
    await expect(bare).toHaveAttribute('target', '_blank');
    await expect(bare).toHaveText(path);

    // fenced code 里的 open <路径>：换成「在浏览器打开 <文件名>」链接。
    const open = answer.locator('[data-kind="local-file-open"] a');
    await expect(open).toHaveCount(1);
    await expect(open).toHaveAttribute('href', href);
    await expect(open).toHaveAttribute('target', '_blank');
    await expect(open).toContainText('在浏览器打开 存钱计划 30 年.html');

    // 普通命令还是代码块，没有被改成路径链接。
    const pre = answer.locator('pre');
    await expect(pre).toHaveCount(1);
    await expect(pre).toContainText('cat /private/tmp/aios-t015/存钱计划 30 年.html | head');
    await expect(pre.locator('a')).toHaveCount(0);

    // 正文里的 URL 也不是本地文件链接。
    await expect(answer.getByRole('link', { name: /example\.com/ })).toHaveCount(0);
  } finally {
    await fixture.close();
  }
});
