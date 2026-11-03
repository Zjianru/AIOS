import { jsx as _jsx } from "react/jsx-runtime";
/**
 * Markdown 渲染（需求 §三.3）：
 * - 每条消息的 markdown 只在内容变化时重新解析：整块用 memo，比较 text 与 elements 的引用；
 * - 引用片以占位符塞进文本，解析后再把占位符换成真组件（这样片子在段落里是内联的）。
 */
import { memo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { QuoteChip } from './QuoteChip';
const TOKEN_SOURCE = '%%Q(\\d+)%%';
function withChips(children, chips) {
    if (chips.size === 0)
        return children;
    const out = [];
    let changed = false;
    const walk = (node) => {
        if (typeof node === 'string') {
            const parts = node.split(new RegExp(TOKEN_SOURCE, 'g'));
            if (parts.length === 1) {
                out.push(node);
                return;
            }
            changed = true;
            for (let i = 0; i < parts.length; i += 1) {
                if (i % 2 === 0) {
                    if (parts[i])
                        out.push(parts[i]);
                }
                else {
                    const element = chips.get(parts[i]);
                    if (element)
                        out.push(_jsx(QuoteChip, { element: element }, 'q' + i + parts[i]));
                }
            }
            return;
        }
        if (Array.isArray(node)) {
            for (const child of node)
                walk(child);
            return;
        }
        out.push(node);
    };
    walk(children);
    return changed ? out : children;
}
function prepare(text, elements) {
    if (!elements || elements.length === 0)
        return { text, chips: new Map() };
    const chips = new Map();
    let out = '';
    let cursor = 0;
    elements.forEach((element, index) => {
        if (element.start < cursor || element.end > text.length || element.end <= element.start)
            return;
        const key = String(index);
        chips.set(key, element);
        out += text.slice(cursor, element.start) + '%%Q' + key + '%%';
        cursor = element.end;
    });
    out += text.slice(cursor);
    return { text: out, chips };
}
function MarkdownBody({ text, elements }) {
    const { text: prepared, chips } = prepare(text, elements);
    if (chips.size === 0) {
        return (_jsx("div", { className: "md", children: _jsx(ReactMarkdown, { remarkPlugins: [remarkGfm], children: prepared }) }));
    }
    const wrap = (tag) => {
        const Wrapped = ({ children }) => {
            const Tag = tag;
            return _jsx(Tag, { children: withChips(children, chips) });
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
    return (_jsx("div", { className: "md", children: _jsx(ReactMarkdown, { remarkPlugins: [remarkGfm], components: components, children: prepared }) }));
}
/** 按 (text, elements) 的引用比较：内容没变就不重解析。 */
export const Markdown = memo(MarkdownBody, (prev, next) => prev.text === next.text && prev.elements === next.elements);
