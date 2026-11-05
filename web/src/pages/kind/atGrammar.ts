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

/** 光标前的活动 @ token；不是 @ 触发位置（比如邮箱）返回 undefined。 */
export function activeAtToken(_line: string, _cursorCol: number): ActiveAtToken | undefined {
  // T013.3 红阶段：实现前空壳。
  return undefined;
}

/** 选中的路径变成提示文本（有空白用 @"..." 形式）；表达不了返回 undefined。 */
export function formatFileMention(_path: string, _kind: 'file' | 'directory', _preserveQuote: boolean): string | undefined {
  // T013.3 红阶段：实现前空壳。
  return undefined;
}
