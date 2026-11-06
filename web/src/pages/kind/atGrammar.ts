/**
 * T013.3：@ 文件语法（照 DSH context/file-reference/src/grammar.ts）。
 * 纯函数，输入框和 @ 菜单共用；不依赖 React/store。
 */

/** 当前光标处的活动 @ token。 */
export interface ActiveAtToken {
  /** 用户接受候选时要整体替换的 token（含 @）。 */
  prefix: string;
  /** @ 或 @" 后面的路径查询（不含开引号）。 */
  query: string;
  /** 用户是否自己打开了引号。 */
  quoted: boolean;
}

/**
 * 光标前的活动 @ token；不是 @ 触发位置（比如邮箱）返回 undefined。
 * @param line 当前编辑行
 * @param cursorCol 光标在该行的列
 */
export function activeAtToken(line: string, cursorCol: number): ActiveAtToken | undefined {
  const beforeCursor = line.slice(0, cursorCol);
  const quoted = /(?:^|\s)(@"([^"]*))$/u.exec(beforeCursor);
  if (quoted?.[1] !== undefined && quoted[2] !== undefined) {
    return { prefix: quoted[1], query: quoted[2], quoted: true };
  }
  const plain = /(?:^|\s)(@([^\s]*))$/u.exec(beforeCursor);
  if (plain?.[1] === undefined || plain[2] === undefined) return undefined;
  return { prefix: plain[1], query: plain[2], quoted: false };
}

/**
 * 选中的路径变成提示文本：有空白用 @"..." 形式，目录保留结尾斜杠；
 * 编辑器表达不了（控制字符/引号）返回 undefined。
 * @param path 候选路径（相对工作目录）
 * @param kind 文件或目录
 * @param preserveQuote 用户自己打开了引号时保持引号
 */
export function formatFileMention(
  path: string,
  kind: 'file' | 'directory',
  preserveQuote: boolean,
): string | undefined {
  const full = kind === 'directory' ? path + '/' : path;
  if (/[\u0000-\u001f\u007f-\u009f"]/u.test(full)) return undefined;
  const quoted = preserveQuote || /\s/u.test(full);
  if (!quoted) return '@' + full;
  if (kind === 'directory') return '@"' + full;
  return '@"' + full + '"';
}
