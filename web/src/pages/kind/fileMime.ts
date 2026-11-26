// 文件名后缀 → MIME 兜底：旧日志的 file 块没有 mime，放回时按名字推回类型（本票 F1①）。
// 认不出返回 null，调用方再退到 text/plain。

const EXTENSION_MIME: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  svg: 'image/svg+xml',
  bmp: 'image/bmp',
  heic: 'image/heic',
  ico: 'image/x-icon',
  txt: 'text/plain',
  text: 'text/plain',
  log: 'text/plain',
  md: 'text/markdown',
  markdown: 'text/markdown',
  json: 'application/json',
  jsonl: 'application/x-ndjson',
  csv: 'text/csv',
  tsv: 'text/tab-separated-values',
  yaml: 'text/yaml',
  yml: 'text/yaml',
  toml: 'text/toml',
  xml: 'text/xml',
  html: 'text/html',
  css: 'text/css',
  js: 'text/javascript',
  ts: 'text/typescript',
  rs: 'text/rust',
  py: 'text/x-python',
  sh: 'text/x-shellscript',
  pdf: 'application/pdf',
};

/** 从文件名推 MIME；不认识返回 null。 */
export function mimeFromFileName(name: string): string | null {
  const dot = name.lastIndexOf('.');
  if (dot <= 0 || dot === name.length - 1) return null;
  const extension = name.slice(dot + 1).toLowerCase();
  return EXTENSION_MIME[extension] ?? null;
}
