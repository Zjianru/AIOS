/**
 * T015 fix B2：回答里的本地绝对路径 → /local-file 链接。
 * 行内 code、整段加粗、正文裸文本（常见扩展名开头的绝对路径）都认；相对路径、URL、代码块里的命令不动。
 * 服务端会 canonicalize 后再校验是不是会话工作目录里的文件，这里只负责生成 href。
 */

/** 一条文本是不是本地绝对路径（含空格的允许；前后空白不算内容）。 */
export function isLocalFilePath(text: string): boolean {
  const value = text.trim();
  if (!value.startsWith('/')) return false;
  return !/\r|\n|\u0000/.test(value);
}

/** 链接地址：路径整体进 query，空格等字符由 encodeURIComponent 编码。 */
export function localFileHref(path: string): string {
  return '/local-file?path=' + encodeURIComponent(path.trim());
}

/** fenced code 只含一行 `open <绝对路径>` 时返回路径；别的命令返回 null（保持代码块原样）。 */
export function openCommandPath(code: string): string | null {
  const trimmed = code.trim();
  if (/\r|\n/.test(trimmed)) return null;
  const match = /^open[ \t]+(\/\S.*)$/.exec(trimmed);
  if (!match) return null;
  const target = match[1].trimEnd();
  // shell 元字符说明这不是「只含 open 路径」，不替用户执行也不做链接。
  if (/[;&|<>$`]/.test(target)) return null;
  return isLocalFilePath(target) ? target : null;
}

/** 回答正文里能点开的常见文件扩展名（小写比较）。 */
const LOCAL_FILE_EXTENSIONS = new Set([
  'html', 'htm', 'xhtml', 'md', 'markdown', 'txt', 'text', 'json', 'jsonl', 'ndjson',
  'csv', 'tsv', 'png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'pdf', 'log', 'yaml', 'yml',
  'toml', 'xml', 'zip',
]);

/** 裸文本路径在正文里的位置（[start, end)，end 开区间）。 */
export interface LocalPathSpan {
  start: number;
  end: number;
  path: string;
}

/** 最长认一段路径：防止正文里孤零零一个 / 一路扫到段尾。 */
const MAX_PATH_LENGTH = 1024;

/** 能开始一条路径：行首、空白、标点或中文旁边都算；紧贴字母数字/斜杠的是分数或 URL 的一段。 */
function startsPath(text: string, index: number): boolean {
  if (index === 0) return true;
  return !/[A-Za-z0-9_/\\]/.test(text[index - 1]);
}

/** 扩展名后面是不是路径边界：行尾、空白、标点、中文都算；字母数字和 _ - / \ 说明路径还没完。 */
function endsPath(text: string, end: number): boolean {
  if (end >= text.length) return true;
  const char = text[end];
  if (/[A-Za-z0-9_/\\-]/.test(char)) return false;
  if (char === '.') {
    // 英文句号结尾算边界；a.html.bak 这种扩展名后面还有名字的算路径没完。
    return !/[A-Za-z0-9]/.test(text[end + 1] ?? '');
  }
  return true;
}

/**
 * 从 start（一个 /）往后找路径结尾：第一个「常见扩展名 + 边界」处收。
 * 空格允许出现在路径里；但空格后面又是 /，说明这是两段话/两条路径，放弃这一段。
 * 换行、超长、扩展名后面还接着路径字符（a.html-backup）都不认。
 */
function pathEndAt(text: string, start: number): number {
  const limit = Math.min(text.length, start + MAX_PATH_LENGTH);
  for (let i = start + 1; i < limit; i += 1) {
    const char = text[i];
    if (char === '\n' || char === '\r') return -1;
    if (char === ' ' || char === '\t') {
      if (text[i + 1] === '/') return -1;
      continue;
    }
    if (char !== '.') continue;
    let end = i + 1;
    while (end < text.length && /[A-Za-z0-9]/.test(text[end])) end += 1;
    if (!LOCAL_FILE_EXTENSIONS.has(text.slice(i + 1, end).toLowerCase())) {
      i = end - 1;
      continue;
    }
    if (endsPath(text, end)) return end;
    i = end - 1;
  }
  return -1;
}

/** 正文里的裸文本绝对路径（不含行内 code / 代码块 / 已有链接：那些由 Markdown 语法自己处理）。 */
export function localPathSpans(text: string): LocalPathSpan[] {
  const spans: LocalPathSpan[] = [];
  let index = 0;
  while (index < text.length) {
    const start = text.indexOf('/', index);
    if (start < 0) break;
    // 路径的第一段按 ASCII 认（private/Users/tmp…）；紧跟中文的 / 多半是「附录/表1.csv」这类相对写法。
    // // 是 URL 的协议头或协议相对地址，/ 后面直接空白是「和/或」这类分隔符，都不是路径。
    if (!startsPath(text, start) || !/[A-Za-z0-9._-]/.test(text[start + 1] ?? '')) {
      index = start + 1;
      continue;
    }
    const end = pathEndAt(text, start);
    if (end < 0) {
      index = start + 1;
      continue;
    }
    spans.push({ start, end, path: text.slice(start, end) });
    index = end;
  }
  return spans;
}

/** 把一段正文切成「文字 / 裸文本路径」交替的片段（空文字片段不返回）。 */
export type LocalPathPiece = string | { path: string };

export function splitLocalPathText(text: string): LocalPathPiece[] {
  const spans = localPathSpans(text);
  if (spans.length === 0) return text ? [text] : [];
  const pieces: LocalPathPiece[] = [];
  let cursor = 0;
  for (const span of spans) {
    if (span.start > cursor) pieces.push(text.slice(cursor, span.start));
    pieces.push({ path: span.path });
    cursor = span.end;
  }
  if (cursor < text.length) pieces.push(text.slice(cursor));
  return pieces;
}

/** 链接文字用的文件名（完整显示，含空格）。 */
export function fileNameOf(path: string): string {
  const trimmed = path.trim().replace(/\/+$/, '');
  const index = trimmed.lastIndexOf('/');
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}
