/**
 * Markdown 渲染（需求 §三.3）：
 * - 每条消息的 markdown 只在内容变化时重新解析：整块用 memo，比较 text 与 elements 的引用；
 * - 引用片以占位符塞进文本，解析后再把占位符换成真组件（这样片子在段落里是内联的）。
 */

import { memo } from 'react';
import type { ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { QuoteChip } from './QuoteChip';
import type { TextElement } from '@/data/types';

const TOKEN_SOURCE = '%%Q(\\d+)%%';

function withChips(children: ReactNode, chips: Map<string, TextElement>): ReactNode {
  if (chips.size === 0) return children;
  const out: ReactNode[] = [];
  let changed = false;
  const walk = (node: ReactNode): void => {
    if (typeof node === 'string') {
      const parts = node.split(new RegExp(TOKEN_SOURCE, 'g'));
      if (parts.length === 1) {
        out.push(node);
        return;
      }
      changed = true;
      for (let i = 0; i < parts.length; i += 1) {
        if (i % 2 === 0) {
          if (parts[i]) out.push(parts[i]);
        } else {
          const element = chips.get(parts[i]);
          if (element) out.push(<QuoteChip key={'q' + i + parts[i]} element={element} />);
        }
      }
      return;
    }
    if (Array.isArray(node)) {
      for (const child of node as ReactNode[]) walk(child);
      return;
    }
    out.push(node);
  };
  walk(children);
  return changed ? out : children;
}

function prepare(text: string, elements: TextElement[] | undefined): { text: string; chips: Map<string, TextElement> } {
  if (!elements || elements.length === 0) return { text, chips: new Map() };
  const chips = new Map<string, TextElement>();
  let out = '';
  let cursor = 0;
  elements.forEach((element, index) => {
    if (element.start < cursor || element.end > text.length || element.end <= element.start) return;
    const key = String(index);
    chips.set(key, element);
    out += text.slice(cursor, element.start) + '%%Q' + key + '%%';
    cursor = element.end;
  });
  out += text.slice(cursor);
  return { text: out, chips };
}

interface MarkdownProps {
  text: string;
  elements?: TextElement[];
}

function MarkdownBody({ text, elements }: MarkdownProps) {
  const { text: prepared, chips } = prepare(text, elements);
  if (chips.size === 0) {
    return (
      <div className="md">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{prepared}</ReactMarkdown>
      </div>
    );
  }
  const wrap = (tag: 'p' | 'li' | 'h1' | 'h2' | 'h3' | 'h4' | 'td' | 'th' | 'strong' | 'em' | 'blockquote') => {
    const Wrapped = ({ children }: { children?: ReactNode }) => {
      const Tag = tag;
      return <Tag>{withChips(children, chips)}</Tag>;
    };
    return Wrapped;
  };
  const components = {
    p: wrap('p'),
    li: wrap('li'),
    h1: wrap('h1'),
    h2: wrap('h2'),
    h3: wrap('h3'),
    h4: wrap('h4'),
    td: wrap('td'),
    th: wrap('th'),
    strong: wrap('strong'),
    em: wrap('em'),
    blockquote: wrap('blockquote'),
  };
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {prepared}
      </ReactMarkdown>
    </div>
  );
}

/** 按 (text, elements) 的引用比较：内容没变就不重解析。 */
export const Markdown = memo(
  MarkdownBody,
  (prev, next) => prev.text === next.text && prev.elements === next.elements,
);
