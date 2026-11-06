/**
 * 存成文件（T015.5）：把画布存成一个能单独打开的 .html。
 *
 * 文件由页面（父窗口）生成：画布 HTML 用日志里这一版的全文（D58 不存画布状态），
 * 基础样式、主题变量和 CSP meta 都来自宿主自己。画布 iframe 不参与生成，也不接受
 * 它发来的 export——那是不可信来源，能伪造文件内容、也能改 Array.prototype.join
 * 拼坏 CSP（T015.5 审计 #2）。
 */

/**
 * 画布文档的基础样式：让不写样式的画布也像 AIOS。
 * 与 public/canvas-frame.html 里的 BASE_STYLE 一致（外壳渲染用同一份视觉）。
 */
export const CANVAS_BASE_STYLE = [
  ':root{color-scheme:light dark}',
  'html,body{margin:0;padding:0;background:transparent}',
  'body{font-family:var(--font-sans);font-size:var(--font-text-md-size);line-height:1.6;',
  'color:var(--color-text-primary);padding:14px 16px;box-sizing:border-box}',
  '*{box-sizing:inherit}',
].join('');

/**
 * 存文件用的 CSP：默认同 /canvas-frame 的响应头，但 frame-ancestors 在 meta 里会被忽略（去掉）；
 * file:// 文档里无 scheme 的主机源会按 file: 继承，所以 CDN/字体源要显式写 https://。
 */
export const CANVAS_EXPORT_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net/npm/ https://unpkg.com https://cdn.tailwindcss.com https://code.jquery.com",
  "style-src 'unsafe-inline' https://fonts.googleapis.com https://cdnjs.cloudflare.com https://cdn.jsdelivr.net/npm/ https://unpkg.com https://cdn.tailwindcss.com https://code.jquery.com",
  'font-src data: https://fonts.gstatic.com',
  'img-src data: blob:',
  'media-src data: blob:',
  'connect-src https://fonts.googleapis.com https://fonts.gstatic.com',
  "frame-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
].join('; ');

export interface CanvasFileParts {
  /** 这一版的标题。 */
  title: string;
  /** 日志里这一版的全文（tool/result.meta.canvas.html）。 */
  html: string;
  /** 当前主题；写在 BASE_STYLE 后面，覆盖它的 color-scheme。 */
  theme: 'dark' | 'light';
  /** MCP Apps 标准名到值的映射，从页面自己的 CSS 变量取。 */
  variables: Record<string, string>;
}

function escapeText(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** 拼出完整文件：CSP meta 和基础样式在 head，画布 HTML 原样放进 body。 */
export function buildCanvasFile({ title, html, theme, variables }: CanvasFileParts): string {
  const inline = Object.entries(variables)
    .filter(([name]) => name.startsWith('--'))
    .map(([name, value]) => name + ':' + value)
    .join(';');
  return (
    '<!doctype html>\n<html lang="zh-CN">\n<head>\n<meta charset="utf-8">\n' +
    '<meta http-equiv="Content-Security-Policy" content="' + CANVAS_EXPORT_CSP + '">\n' +
    '<title>' + escapeText(title) + '</title>\n' +
    '<style>' + CANVAS_BASE_STYLE + ':root{color-scheme:' + theme + (inline ? ';' + inline : '') + '}</style>\n' +
    '</head>\n<body>' + html + '</body>\n</html>\n'
  );
}
