// T078.2：从 kind_say/kind_react 的工具参数派生页面显示；参数增量走现有 tool-call-delta。
// 参数用 Vercel AI SDK 的 fixJson/parsePartialJson 解析，最后一条没写完也跟着变长。

import { parsePartialJson } from './parsePartialJson.ts';

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
    return value.parts.filter(
      (part): part is string => typeof part === 'string' && part.trim() !== '',
    );
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

/** 流式 kind_say：当前这条写到一半也取出来，跟着增量变长。 */
export function partialKindSayParts(argumentsJson: string): string[] {
  const parsed = parsePartialJson(argumentsJson) as { parts?: unknown } | undefined;
  if (!Array.isArray(parsed?.parts)) return [];
  return parsed.parts.filter(
    (part): part is string => typeof part === 'string' && part.trim() !== '',
  );
}

/** 流式 kind_react：emoji_id 已经写全就显示。 */
export function partialKindReactId(argumentsJson: string): string | null {
  const parsed = parsePartialJson(argumentsJson) as { emoji_id?: unknown } | undefined;
  return typeof parsed?.emoji_id === 'string' ? parsed.emoji_id : null;
}
