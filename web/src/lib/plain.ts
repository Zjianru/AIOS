/** 把 Markdown 摘要显示成纯文字（H1–H6、粗斜体、行内代码、代码围栏、列表标记、引用、链接）。 */
export function plainText(markdown: string): string {
  return markdown
    .replace(/\u0060\u0060\u0060[\s\S]*?\u0060\u0060\u0060/g, ' ')
    .replace(/\u0060([^\u0060]+)\u0060/g, '$1')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}#{1,6}\s+/gm, '')
    .replace(/^\s{0,3}>\s?/gm, '')
    .replace(/^\s{0,3}(?:[-*+]|\d+\.)\s+/gm, '')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/__([^_]+)__/g, '$1')
    .replace(/\*([^*\n]+)\*/g, '$1')
    .replace(/_([^_\n]+)_/g, '$1')
    .replace(/~~([^~]+)~~/g, '$1')
    .replace(/\s+/g, ' ')
    .trim();
}
