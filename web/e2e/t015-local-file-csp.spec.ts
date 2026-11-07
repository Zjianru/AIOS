import { expect, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { startKernelFixture, type KernelFixture } from './service-fixture';

// T015 安全修复：/local-file 的隔离 CSP（Claude 2026-11-07 定案，照 Jupyter 的
// AuthenticatedFileHandler：frame-ancestors 'self'; sandbox allow-scripts）。
//
// 这里全部走真 serve + 真 /local-file 响应头 + 真 Chrome 上下文：
// - HTML/SVG 的内联脚本要能跑（allow-scripts），但进 opaque origin：读不到 cookie /
//   同源存储，fetch /api 与同目录文件都失败；
// - PDF 不能因为加了 CSP 变成下载（Chrome 内嵌查看器）；
// - txt/md/png 照常显示。
const CSP = "frame-ancestors 'self'; sandbox allow-scripts";

function localFileUrl(fixture: KernelFixture, file: string): string {
  return fixture.baseUrl + '/local-file?path=' + encodeURIComponent(file);
}

// 先照用户打开服务地址的路径换 cookie，再顶层导航到 /local-file。
// secret 是探针要试读的同目录另一个文件；/local-file 只认 path，多余查询参数忽略。
async function openLocalFile(
  page: any,
  fixture: KernelFixture,
  file: string,
  secret?: string,
) {
  await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token));
  const url = localFileUrl(fixture, file) + (secret ? '&secret=' + encodeURIComponent(secret) : '');
  const response = await page.goto(url);
  if (!response) throw new Error('没有拿到 /local-file 响应');
  return response;
}

// 服务端原始响应头（PDF 的 page.goto 拿到的是 Chrome PDF 查看器派生后的响应，
// CSP 会被浏览器改写成只剩 frame-ancestors，不能用它核服务端原串）。
async function cookieOf(request: any, fixture: KernelFixture): Promise<string> {
  const connect = await request.get(
    fixture.baseUrl + '/connect?token=' + encodeURIComponent(fixture.token),
    { maxRedirects: 0 },
  );
  const setCookie = connect.headers()['set-cookie'];
  if (!setCookie) throw new Error('connect 没有返回 set-cookie');
  return setCookie.split(';', 1)[0];
}

interface ProbeResult {
  scriptRan: boolean;
  cookie: string;
  localStorage: string;
  api: string;
  localFile: string;
  canvas?: string;
}

// 恶意 HTML：内联脚本既做攻击探针，也画一个 canvas（证明正常脚本/图表没被打坏）。
function probeHtml(): string {
  return [
    '<!doctype html>',
    '<html><body>',
    '<h1>csp probe</h1>',
    '<div id="marker">script-not-run</div>',
    '<canvas id="chart" width="60" height="60"></canvas>',
    '<pre id="result">pending</pre>',
    '<script>',
    '(async () => {',
    '  const secret = new URLSearchParams(location.search).get("secret");',
    '  const out = { scriptRan: true, cookie: "unset", localStorage: "unset", api: "unset", localFile: "unset", canvas: "unset" };',
    '  try { out.cookie = document.cookie; } catch (error) { out.cookie = "THROW:" + error.name; }',
    '  try { localStorage.setItem("probe", "1"); out.localStorage = "OK"; } catch (error) { out.localStorage = "THROW:" + error.name; }',
    '  try {',
    '    const response = await fetch("/api", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });',
    '    out.api = "STATUS:" + response.status + ":" + (await response.text()).slice(0, 40);',
    '  } catch (error) { out.api = "REJECT:" + error.name; }',
    '  try {',
    '    const response = await fetch("/local-file?path=" + encodeURIComponent(secret));',
    '    out.localFile = "STATUS:" + response.status + ":" + (await response.text()).slice(0, 40);',
    '  } catch (error) { out.localFile = "REJECT:" + error.name; }',
    '  try {',
    '    const canvas = document.getElementById("chart");',
    '    const context = canvas.getContext("2d");',
    '    context.fillStyle = "#ff0000";',
    '    context.fillRect(0, 0, 30, 30);',
    '    out.canvas = Array.from(context.getImageData(5, 5, 1, 1).data).join(",");',
    '  } catch (error) { out.canvas = "REJECT:" + error.name; }',
    '  document.getElementById("marker").textContent = "SCRIPT-RAN";',
    '  document.getElementById("result").textContent = JSON.stringify(out);',
    '})();',
    '</script>',
    '</body></html>',
  ].join('\n');
}

// 恶意 SVG：顶层直接打开（不是 img），内嵌脚本要受同一套 sandbox 约束。
function probeSvg(): string {
  return [
    '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="240">',
    '<rect width="100%" height="100%" fill="#ffffff"/>',
    '<text id="marker" x="20" y="40" font-size="20">script-not-run</text>',
    '<text id="result" x="20" y="80" font-size="14">pending</text>',
    '<script><![CDATA[',
    '(async () => {',
    '  const secret = new URLSearchParams(location.search).get("secret");',
    '  const out = { scriptRan: true, cookie: "unset", api: "unset", localFile: "unset" };',
    '  try { out.cookie = document.cookie; } catch (error) { out.cookie = "THROW:" + error.name; }',
    '  try {',
    '    const response = await fetch("/api", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });',
    '    out.api = "STATUS:" + response.status + ":" + (await response.text()).slice(0, 40);',
    '  } catch (error) { out.api = "REJECT:" + error.name; }',
    '  try {',
    '    const response = await fetch("/local-file?path=" + encodeURIComponent(secret));',
    '    out.localFile = "STATUS:" + response.status + ":" + (await response.text()).slice(0, 40);',
    '  } catch (error) { out.localFile = "REJECT:" + error.name; }',
    '  document.getElementById("marker").textContent = "SVG-SCRIPT-RAN";',
    '  document.getElementById("result").textContent = JSON.stringify(out);',
    '})();',
    ']]></script>',
    '</svg>',
  ].join('\n');
}

// 最小可渲染 PDF（Chrome 打开后能看到 PDF-OK）。
function minimalPdf(): Buffer {
  const objects: string[] = [];
  objects.push('<</Type/Catalog/Pages 2 0 R>>');
  objects.push('<</Type/Pages/Kids[3 0 R]/Count 1>>');
  objects.push('<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 300]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>');
  const stream = 'BT /F1 24 Tf 50 150 Td (PDF-OK) Tj ET';
  objects.push('<</Length ' + stream.length + '>>stream\n' + stream + '\nendstream');
  objects.push('<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>');
  let pdf = '%PDF-1.4\n';
  const offsets = [0];
  for (let index = 0; index < objects.length; index += 1) {
    offsets.push(pdf.length);
    pdf += String(index + 1) + ' 0 obj' + objects[index] + 'endobj\n';
  }
  const xref = pdf.length;
  pdf += 'xref\n0 ' + String(objects.length + 1) + '\n0000000000 65535 f \n';
  for (let index = 1; index <= objects.length; index += 1) {
    pdf += String(offsets[index]).padStart(10, '0') + ' 00000 n \n';
  }
  pdf += 'trailer<</Size ' + String(objects.length + 1) + '/Root 1 0 R>>\nstartxref\n' + String(xref) + '\n%%EOF\n';
  return Buffer.from(pdf, 'latin1');
}

async function resultOf(page: any): Promise<ProbeResult> {
  await expect(page.locator('#result')).not.toHaveText('pending', { timeout: 20_000 });
  return JSON.parse((await page.locator('#result').textContent()) ?? '{}') as ProbeResult;
}

test('本地文件 CSP：HTML 脚本能跑，但拿不到 cookie/同源存储，/api 和同目录文件都调不通', async ({ page }) => {
  const fixture = await startKernelFixture({});
  try {
    const secret = path.join(fixture.dataDir, 'csp-secret.txt');
    await writeFile(secret, 'CSP-SECRET-CONTENT', 'utf8');
    const probe = path.join(fixture.dataDir, 'csp-probe.html');
    await writeFile(probe, probeHtml(), 'utf8');

    const response = await openLocalFile(page, fixture, probe, secret);
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toContain('text/html');

    // 正常脚本照跑：改了 DOM，canvas 也画上了（图表类用法没被 CSP 打坏）。
    await expect(page.locator('#marker')).toHaveText('SCRIPT-RAN');
    const result = await resultOf(page);
    expect(result.scriptRan).toBe(true);
    expect(result.canvas).toBe('255,0,0,255');

    // 恶意脚本被隔离。用 soft：红日志能一次看到四条都没被拦住。
    expect.soft(result.cookie, 'sandbox 页面不能读到 AIOS cookie').toBe('THROW:SecurityError');
    expect.soft(result.localStorage, 'opaque origin 拿不到同源存储').toBe('THROW:SecurityError');
    expect.soft(result.api, 'fetch /api 必须被拒').toMatch(/^REJECT:/);
    expect.soft(result.localFile, 'fetch 同目录其他文件必须被拒').toMatch(/^REJECT:/);
    expect.soft(response.headers()['content-security-policy'], '响应头逐字照定案').toBe(CSP);
  } finally {
    await fixture.close();
  }
});

test('本地文件 CSP：直接打开 SVG 时内嵌脚本同样被 sandbox 隔离（脚本能跑、调不通 /api）', async ({ page }) => {
  const fixture = await startKernelFixture({});
  try {
    const secret = path.join(fixture.dataDir, 'svg-secret.txt');
    await writeFile(secret, 'SVG-SECRET-CONTENT', 'utf8');
    const probe = path.join(fixture.dataDir, 'csp-probe.svg');
    await writeFile(probe, probeSvg(), 'utf8');

    const response = await openLocalFile(page, fixture, probe, secret);
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toContain('image/svg+xml');

    // 顶层 SVG 的内嵌脚本能跑（没有被 CSP 整个掐掉），这是「隔离而不是禁用」。
    await expect(page.locator('#marker')).toHaveText('SVG-SCRIPT-RAN');
    const result = await resultOf(page);
    expect(result.scriptRan).toBe(true);

    expect.soft(result.cookie, 'sandbox 里的 SVG 不能读到 AIOS cookie').toBe('THROW:SecurityError');
    expect.soft(result.api, 'SVG 里 fetch /api 必须被拒').toMatch(/^REJECT:/);
    expect.soft(result.localFile, 'SVG 里 fetch 同目录其他文件必须被拒').toMatch(/^REJECT:/);
    expect.soft(response.headers()['content-security-policy'], 'SVG 响应同样是这条 CSP').toBe(CSP);
  } finally {
    await fixture.close();
  }
});

test('本地文件 CSP：PDF 仍交给 Chrome 内嵌查看器，不变成下载', async ({ page, request }) => {
  const fixture = await startKernelFixture({});
  try {
    const probe = path.join(fixture.dataDir, 'csp-probe.pdf');
    await writeFile(probe, minimalPdf());

    // 服务端原始响应：PDF 也带同一条 CSP，且没有变成 attachment。
    const raw = await request.get(localFileUrl(fixture, probe), {
      headers: { Cookie: await cookieOf(request, fixture) },
    });
    expect(raw.status()).toBe(200);
    expect(raw.headers()['content-type']).toBe('application/pdf');
    expect(raw.headers()['content-security-policy']).toBe(CSP);
    expect(raw.headers()['content-disposition'] ?? '').not.toContain('attachment');
    expect(raw.headers()['cache-control']).toBe('no-store');
    expect(raw.headers()['x-content-type-options']).toBe('nosniff');

    const downloads: string[] = [];
    page.on('download', (download: any) => downloads.push(download.suggestedFilename()));
    const response = await openLocalFile(page, fixture, probe);
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toBe('application/pdf');
    expect(response.headers()['content-disposition'] ?? '').not.toContain('attachment');

    // Chrome 自带的 PDF 查看器（扩展页）接住了导航；没有 download 事件。
    await expect
      .poll(() => page.frames().some((frame: any) => frame.url().startsWith('chrome-extension://')), {
        timeout: 10_000,
      })
      .toBe(true);
    await expect.poll(() => page.evaluate(() => document.contentType), { timeout: 10_000 }).toBe('application/pdf');
    expect(downloads, 'PDF 不能变成下载').toEqual([]);
    // 顶层导航本身仍是 200 + application/pdf（上面 response 的断言），浏览器侧没有下载。
  } finally {
    await fixture.close();
  }
});

test('本地文件 CSP：txt、md、png 照常显示', async ({ page }) => {
  const fixture = await startKernelFixture({});
  try {
    const text = path.join(fixture.dataDir, 'csp-note.txt');
    await writeFile(text, 'TXT-CSP-OK', 'utf8');
    const markdown = path.join(fixture.dataDir, 'csp-note.md');
    await writeFile(markdown, '# MD-CSP-OK', 'utf8');
    const pixel = path.join(fixture.dataDir, 'csp-pixel.png');
    await writeFile(
      pixel,
      Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
        'base64',
      ),
    );

    await page.goto(fixture.baseUrl + '/?token=' + encodeURIComponent(fixture.token));
    await page.goto(localFileUrl(fixture, text));
    expect(await page.locator('body').innerText()).toContain('TXT-CSP-OK');

    await page.goto(localFileUrl(fixture, markdown));
    expect(await page.locator('body').innerText()).toContain('# MD-CSP-OK');

    await page.goto(localFileUrl(fixture, pixel));
    expect(await page.evaluate(() => document.contentType)).toBe('image/png');
    expect(await page.evaluate(() => document.querySelector('img')?.naturalWidth ?? 0)).toBe(1);
  } finally {
    await fixture.close();
  }
});
