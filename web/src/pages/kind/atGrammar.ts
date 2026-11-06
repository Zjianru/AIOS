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

/** T013 验收 F6：消息里 @文件引用 token 的位置（含 @；带空格的路径可以是 @"..."）。 */
export interface FileMention {
  /** 原文（含 @，带引号时含引号）。 */
  raw: string;
  /** 路径（去掉 @ 和引号）。 */
  path: string;
  /** 在整段文字里的起止下标（raw 的范围，含 @）。 */
  start: number;
  end: number;
}

/**
 * 认 @路径 / @"带 空格 的路径"。只认行首/空白/常见标点后面的 @（邮箱里的 @ 不算），
 * 没引号的路径在中文标点处停（@hello.txt，谢谢 → hello.txt）。
 */
const FILE_MENTION = /(^|[\s(（【「『，。；：、])@(?:"([^"\n]*)"|([^\s@"，。；：！？、（）【】「」『』]+))/gu;

/** 纯函数：把一段文字切成「普通文字 / @文件引用」用的 token 列表。 */
export function fileMentions(text: string): FileMention[] {
  const out: FileMention[] = [];
  for (const match of text.matchAll(FILE_MENTION)) {
    const prefix = match[1] ?? '';
    const raw = match[0].slice(prefix.length);
    const path = match[2] ?? match[3] ?? '';
    if (!path) continue;
    const start = (match.index ?? 0) + prefix.length;
    out.push({ raw, path, start, end: start + raw.length });
  }
  return out;
}

