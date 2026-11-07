/**
 * Markdown 渲染（需求 §三.3）：
 * - 每条消息的 markdown 只在内容变化时重新解析：整块用 memo，比较 text 与 elements 的引用；
 * - 引用片以占位符塞进文本，解析后再把占位符换成真组件（这样片子在段落里是内联的）。
 */

import { memo, useRef } from 'react';
import type { ReactNode } from 'react';
import { CopyButton } from './CopyButton';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { QuoteChip } from './QuoteChip';
import { fileNameOf, isLocalFilePath, localFileHref, openCommandPath } from './localFile';
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

type MdNode = {
  type?: string;
  value?: string;
  url?: string;
  title?: string | null;
  children?: MdNode[];
};

function nodeText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join('');
  if (node && typeof node === 'object' && 'props' in node) {
    const props = (node as { props?: { children?: ReactNode } }).props;
    return nodeText(props?.children ?? null);
  }
  return '';
}

/** T015 fix B2：行内 code 的绝对路径、整段加粗的绝对路径 → /local-file 链接（含空格路径）。 */
function rewriteLocalPaths(node: MdNode): void {
  if (!node.children) return;
  node.children = node.children.map((child) => {
    if (child.type === 'inlineCode') {
      const value = child.value ?? '';
      if (isLocalFilePath(value)) {
        const path = value.trim();
        return {
          type: 'link',
          url: localFileHref(path),
          title: null,
          children: [{ type: 'text', value: path }],
        };
      }
    }
    if (child.type === 'strong') {
      const text = nodeText(child as unknown as ReactNode).trim();
      if (isLocalFilePath(text)) {
        return {
          type: 'link',
          url: localFileHref(text),
          title: null,
          children: child.children ?? [{ type: 'text', value: text }],
        };
      }
    }
    rewriteLocalPaths(child);
    return child;
  });
}

function localFileLinks() {
  return (tree: MdNode) => rewriteLocalPaths(tree);
}

function MdLink({
  href,
  children,
  chips,
}: {
  href?: string;
  children?: ReactNode;
  chips?: Map<string, TextElement>;
}) {
  const content = chips ? withChips(children, chips) : children;
  if (href && href.startsWith('/local-file?')) {
    return (
      <a href={href} target="_blank" rel="noreferrer noopener" data-kind="local-file-link">
        {content}
      </a>
    );
  }
  return <a href={href}>{content}</a>;
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
        <ReactMarkdown
          remarkPlugins={[remarkGfm, localFileLinks]}
          components={{ pre: PreBlock, a: MdLink }}
        >
          {prepared}
        </ReactMarkdown>
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
    pre: PreBlock,
    a: (props: { href?: string; children?: ReactNode }) => <MdLink {...props} chips={chips} />,
  };
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm, localFileLinks]} components={components}>
        {prepared}
      </ReactMarkdown>
    </div>
  );
}

/** T013.6：代码块右上角的复制（复制原文；从 DOM 里取 code 的 textContent）。 */
function PreBlock({ children }: { children?: ReactNode }) {
  const ref = useRef<HTMLPreElement | null>(null);
  // T015 fix B2：只含一行 `open <绝对路径>` 的 fenced code 换成可点链接；普通代码/命令不变。
  const openTarget = openCommandPath(nodeText(children));
  if (openTarget) {
    return (
      <p data-kind="local-file-open" className="my-1">
        <a
          href={localFileHref(openTarget)}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 text-run underline underline-offset-2 hover:text-ink"
        >
          在浏览器打开 {fileNameOf(openTarget)}
        </a>
      </p>
    );
  }
  const copyText = () => {
    const code = ref.current?.querySelector('code');
    return code?.textContent ?? '';
  };
  return (
    <div className="group/code relative">
      <pre ref={ref}>{children}</pre>
      <CopyButton kind="code-copy" text={copyText} className="absolute right-1.5 top-1.5" />
    </div>
  );
}

/** 按 (text, elements) 的引用比较：内容没变就不重解析。 */
export const Markdown = memo(
  MarkdownBody,
  (prev, next) => prev.text === next.text && prev.elements === next.elements,
);
