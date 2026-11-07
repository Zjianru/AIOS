import { expect, request as playwrightRequest, test } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture } from './service-fixture';

/**
 * T015 fix B1：认证的本地文件路由。只读、只放会话工作目录里的普通文件；
 * 同源 cookie + Host 才给，越界 403，目录/坏路径 404，没 cookie 401。
 */
async function cookieOf(request: any, fixture: { baseUrl: string; token: string }) {
  const connect = await request.get(
    fixture.baseUrl + '/connect?token=' + encodeURIComponent(fixture.token),
    { maxRedirects: 0 },
  );
  const setCookie = connect.headers()['set-cookie'];
  if (!setCookie) throw new Error('connect 没有返回 set-cookie');
  return setCookie.split(';', 1)[0];
}

test('本地文件路由：会话工作目录内的文件可读，越界/目录/坏路径被拒', async ({ request }) => {
  const fixture = await startKernelFixture({});
  try {
    const answer = path.join(fixture.dataDir, 'answer.html');
    await writeFile(answer, '<!doctype html><h1>本地文件正文</h1>', 'utf8');
    const spaced = path.join(fixture.dataDir, '报告 第 1 版 空格.html');
    await writeFile(spaced, '<!doctype html><h1>带空格和中文</h1>', 'utf8');
    const nested = path.join(fixture.dataDir, 'nested');
    await mkdir(nested);
    const cookie = await cookieOf(request, fixture);

    const get = (target: string, headers: Record<string, string> | undefined = { Cookie: cookie }) =>
      request.get(fixture.baseUrl + '/local-file?path=' + encodeURIComponent(target), { headers });

    const ok = await get(answer);
    expect(ok.status()).toBe(200);
    expect(ok.headers()['content-type']).toContain('text/html');
    expect(await ok.text()).toContain('本地文件正文');

    // 空格/中文文件名：前端整体 encodeURIComponent，服务端 Query 解回原路径。
    const unicode = await get(spaced);
    expect(unicode.status()).toBe(200);
    expect(await unicode.text()).toContain('带空格和中文');

    // 越界（会话工作目录之外）403，目录/不存在的文件 404，相对路径 400。
    expect((await get('/etc/hosts')).status()).toBe(403);
    expect((await get(fixture.dataDir)).status()).toBe(404);
    expect((await get(nested)).status()).toBe(404);
    expect((await get(path.join(fixture.dataDir, 'missing.html'))).status()).toBe(404);
    expect((await get('answer.html')).status()).toBe(400);
    // 没 cookie 401；token 不能从 query 冒充认证。
    // 注意：Playwright 的 request 上下文会把 /connect 的 cookie 存进自己的 jar，
    // 传 headers: {} 并不会去掉它；要测「没 cookie」必须开一个空 context。
    const anonymous = await playwrightRequest.newContext();
    try {
      const noCookie = await anonymous.get(
        fixture.baseUrl + '/local-file?path=' + encodeURIComponent(answer),
      );
      expect(noCookie.status()).toBe(401);
    } finally {
      await anonymous.dispose();
    }
    expect(
      (await request.get(fixture.baseUrl + '/local-file?path=' + encodeURIComponent(answer) + '&token=' + encodeURIComponent(fixture.token))).status(),
    ).toBe(401);
  } finally {
    await fixture.close();
  }
});
