/**
 * T013 验收 F5：从会话事件里取 reasoning（纯函数，不依赖 React/store）。
 *
 * 事实来自真实验收会话 7b1f6f0d-ec59-4f92-9df7-2050d9e2688e.jsonl：
 * - assistant/message 的 content 里是 { type: 'reasoning', text, signature }（思考全文）；
 * - 同一事件的 stream 里有 { type: 'reasoning-chunks', time0, dt, texts }（增量；时长由 dt 算）；
 * - reasoning 块没有 summary 字段，日志里也不带摘要，页面不编摘要。
 */

/** assistant/message.content 里 reasoning 块的全文（多块之间空一行）。 */
export function reasoningTextOf(message: unknown): string {
  if (!message || typeof message !== 'object') return '';
  const content = (message as { content?: unknown }).content;
  if (!Array.isArray(content)) return '';
  const blocks: string[] = [];
  for (const block of content) {
    if (!block || typeof block !== 'object') continue;
    const typed = block as { type?: unknown; text?: unknown };
    if (typed.type !== 'reasoning' || typeof typed.text !== 'string') continue;
    const text = typed.text.trim();
    if (text) blocks.push(text);
  }
  return blocks.join('\n\n');
}

/** stream 里 reasoning-chunks 的增量全文（assistant/attempt 只有 stream 时用）。 */
export function reasoningTextOfStream(stream: unknown): string {
  if (!Array.isArray(stream)) return '';
  const runs: string[] = [];
  for (const record of stream) {
    if (!record || typeof record !== 'object') continue;
    const typed = record as { type?: unknown; texts?: unknown };
    if (typed.type !== 'reasoning-chunks' || !Array.isArray(typed.texts)) continue;
    const text = typed.texts.filter((item): item is string => typeof item === 'string').join('');
    if (text.trim()) runs.push(text.trim());
  }
  return runs.join('\n\n');
}
