/**
 * T015 fix B2：回答里的本地绝对路径 → /local-file 链接。
 * 只认以 / 开头的单行路径；相对路径、URL、命令和代码块里的普通命令都不动。
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

/** 链接文字用的文件名（完整显示，含空格）。 */
export function fileNameOf(path: string): string {
  const trimmed = path.trim().replace(/\/+$/, '');
  const index = trimmed.lastIndexOf('/');
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}
