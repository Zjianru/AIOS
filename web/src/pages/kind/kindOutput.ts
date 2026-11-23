// T078.2：从 kind_say/kind_react 的工具参数派生页面显示；参数增量走现有 tool-call-delta。
// parsePartialJson 思路照 Vercel AI SDK（Apache-2.0），只用来取已成形的前几条。

export const KIND_EMOJI_CHARS: Record<string, string> = {
  '+1': '👍',
  ok_hand: '👌',
  eyes: '👀',
  pray: '🙏',
  heart: '❤️',
  tada: '🎉',
  cry: '😢',
  sweat_smile: '😅',
  saluting_face: '🫡',
  yawning_face: '🥱',
  joy: '😂',
};

export function kindEmojiChar(id: string): string {
  return KIND_EMOJI_CHARS[id] ?? id;
}

export function parseKindSayParts(argumentsJson: string): string[] {
  try {
    const value = JSON.parse(argumentsJson) as { parts?: unknown };
    if (!Array.isArray(value.parts)) return [];
    return value.parts.filter((part): part is string => typeof part === 'string' && part.trim() !== '');
  } catch {
    return [];
  }
}

export function parseKindReactId(argumentsJson: string): string | null {
  try {
    const value = JSON.parse(argumentsJson) as { emoji_id?: unknown };
    return typeof value.emoji_id === 'string' ? value.emoji_id : null;
  } catch {
    return null;
  }
}

function repairJson(text: string): string {
  let value = text.trim().replace(/,$/, '');
  const stack: string[] = [];
  let inString = false;
  let escaped = false;
  for (const char of value) {
    if (inString) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') inString = true;
    else if (char === '[') stack.push(']');
    else if (char === '{') stack.push('}');
    else if (char === ']' || char === '}') stack.pop();
  }
  if (inString) value += '"';
  value = value.replace(/,s*$/, '');
  while (stack.length > 0) value += stack.pop();
  return value;
}

/** 尽量解析还正在写的 JSON：取最后能解析的前缀并补全括号。 */
export function parsePartialJson(text: string | undefined): unknown {
  if (!text) return undefined;
  for (let end = text.length; end > 0; end -= 1) {
    try {
      return JSON.parse(repairJson(text.slice(0, end)));
    } catch {
      // 继续缩短前缀。
    }
  }
  return undefined;
}

/** 流式 kind_say：只取已经完整写出的 parts。 */
export function partialKindSayParts(argumentsJson: string): string[] {
  const key = argumentsJson.indexOf('"parts"');
  if (key < 0) return [];
  const tail = argumentsJson.slice(key + '"parts"'.length);
  const parts: string[] = [];
  for (const match of tail.matchAll(/"((?:\\.|[^"\\])*)"/g)) {
    try {
      const part = JSON.parse('"' + match[1] + '"') as string;
      if (part.trim() !== '') parts.push(part);
    } catch {
      // 不完整的转义先跳过，等下一帧。
    }
  }
  return parts;
}

/** 流式 kind_react：emoji_id 已经写全就显示。 */
export function partialKindReactId(argumentsJson: string): string | null {
  const parsed = parsePartialJson(argumentsJson) as { emoji_id?: unknown } | undefined;
  return typeof parsed?.emoji_id === 'string' ? parsed.emoji_id : null;
}
