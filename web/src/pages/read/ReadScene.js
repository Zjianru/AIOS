import { Fragment as _Fragment, jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
/**
 * 阅读页（#read:<文件名>；§8.6 资料与成果共用；brief §四「成果、阅读页、查看器」）。
 *
 * 三栏：左目录 · 中原件（按类型渲染）· 右栏（伴读 / 批注 / 信息，进入验收时换成验收面板）。
 * 顶部：名称、来源、版本、状态、打开原件、在 Finder 中显示；下面一条横幅只说最要紧的事。
 * 数据只从 api 取（getRead）；原型里除「结构说明.docx」用了一版真实文案，其余是占位内容。
 */
import { useEffect, useState } from 'react';
import { create } from 'zustand';
import { ArrowLeft, MapPin, SendHorizontal } from 'lucide-react';
import { toast } from 'sonner';
import { getRead } from '@/data/api';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { cn } from '@/lib/utils';
/** 阅读页右栏的页签：横幅上的「开始验收」也要能把右栏切过去，所以放一个小 store（按文件重置）。 */
const useReadTab = create()((set) => ({
    tab: 'companion',
    setTab: (tab) => set({ tab }),
    reset: () => set({ tab: 'companion' }),
}));
/** v13-o O2 的 diff 查看器：并排列号 + 增删改颜色（用状态色，不另发明颜色）。 */
function DiffViewer({ doc, onClose }) {
    if (!doc.diff)
        return null;
    const tone = {
        same: 'bg-transparent',
        chg: 'bg-wait-soft',
        add: 'bg-done-soft',
        del: 'bg-fail-soft',
    };
    return (_jsxs("div", { className: "mt-3 overflow-hidden rounded-card border border-line", children: [_jsxs("div", { className: "flex items-center gap-2 border-b border-line bg-pane px-2.5 py-1.5 text-2", children: [_jsx("span", { className: "text-ink-2", children: "\u5BF9\u6BD4\u4E0A\u4E00\u7248" }), _jsxs("span", { className: "text-1 text-ink-3", children: ["v1 \u2192 ", doc.version, " \u00B7 \u6539\u52A8\u5904\u7528\u989C\u8272\u6807\u51FA\u6765\uFF08\u539F\u578B\u91CC\u662F\u5199\u597D\u7684\u6837\u4F8B\uFF09"] }), _jsx("button", { type: "button", onClick: onClose, className: "ml-auto rounded-btn border border-line-2 px-2 py-[2px] text-1 text-ink-2 transition-colors hover:bg-hover", children: "\u5173\u95ED\u5BF9\u6BD4" })] }), _jsx("div", { className: "grid grid-cols-2 divide-x divide-line text-1 leading-5", children: ['left', 'right'].map((side) => (_jsxs("div", { className: "min-w-0", children: [_jsx("div", { className: "border-b border-line bg-pane px-2 py-1 text-ink-3", children: side === 'left' ? 'v1（上一版）' : doc.version + '（当前）' }), doc.diff[side].map((line, index) => (_jsxs("div", { className: cn('flex gap-2 px-2 py-[2px] font-mono text-2', tone[doc.diff.status[index] ?? 'same']), children: [_jsx("span", { className: "w-5 shrink-0 text-right brand-type text-ink-3", children: index + 1 }), _jsx("span", { className: "min-w-0 break-all text-ink-2", children: line })] }, side + index)))] }, side))) })] }));
}
const TYPE_COLOR = {
    docx: 'var(--line-w)',
    pdf: 'var(--fail)',
    xlsx: 'var(--done)',
    md: 'var(--line-n)',
    png: 'var(--line-d)',
    graph: 'var(--line-d)',
    json: 'var(--wait)',
    html: 'var(--line-n)',
    sh: 'var(--ink-3)',
};
function typeLabel(type) {
    return (type || 'file').toUpperCase();
}
function Banner({ doc }) {
    const setTab = useReadTab((state) => state.setTab);
    if (doc.status === 'pend') {
        return (_jsxs("div", { className: "mt-3 flex flex-wrap items-center gap-2 rounded-card border border-wait/40 bg-wait-soft/50 px-3 py-2 text-2 text-wait", children: [_jsxs("span", { children: ["\u8FD9\u4EFD\u6210\u679C\u7B49\u4F60\u9A8C\u6536 \u00B7 \u5B8C\u6210\u6807\u51C6 ", doc.criteria.length, " \u6761"] }), _jsx("button", { type: "button", onClick: () => setTab('accept'), className: "ml-auto rounded-btn bg-wait px-2.5 py-1 text-1 font-semibold text-content", children: "\u5F00\u59CB\u9A8C\u6536" })] }));
    }
    if (doc.status === 'acc') {
        return (_jsxs("div", { className: "mt-3 rounded-card border border-done/40 bg-done-soft/50 px-3 py-2 text-2 text-done", children: ["\u2713\u2713 \u5DF2\u63A5\u53D7 ", doc.version, " \u00B7 \u8BB0\u5F55\u5728\u9A8C\u6536\u9762\u677F\u91CC\uFF1B\u4E4B\u540E\u518D\u6709\u65B0\u7248\u672C\u8981\u91CD\u65B0\u63A5\u53D7\u3002"] }));
    }
    if (doc.status === 'bad') {
        return (_jsx("div", { className: "mt-3 rounded-card border border-fail/40 bg-fail-soft/50 px-3 py-2 text-2 text-fail", children: "\u6253\u4E0D\u5F00 \u00B7 \u73B0\u5728\u663E\u793A\u7684\u662F\u65E7\u5FEB\u7167\uFF0C\u4E0D\u518D\u5F53\u4F5C\u6700\u65B0\u5185\u5BB9\u3002" }));
    }
    if (doc.status === 'busy') {
        return (_jsx("div", { className: "mt-3 rounded-card border border-wait/40 bg-wait-soft/50 px-3 py-2 text-2 text-wait", children: "\u6B63\u5728\u6574\u7406 \u00B7 \u73B0\u5728\u80FD\u8BFB\uFF0C\u4F46\u8FD8\u641C\u4E0D\u5168\u3002" }));
    }
    if (doc.origin === 'artifact') {
        return _jsx("div", { className: "mt-3 rounded-card border border-line bg-pane/60 px-3 py-2 text-2 text-ink-3", children: "\u8349\u7A3F \u00B7 \u8FD8\u6CA1\u63D0\u4EA4\u9A8C\u6536\u3002" });
    }
    return null;
}
function SectionBody({ section }) {
    return (_jsxs("div", { className: "mt-5 first:mt-0", children: [section.heading ? _jsx("h3", { className: "text-4 font-medium leading-6 text-ink", children: section.heading }) : null, section.paragraphs.map((paragraph) => (_jsx("p", { className: "mt-2 text-3 leading-6 text-ink-2", children: paragraph }, paragraph))), section.bullets?.length ? (_jsx("ul", { className: "mt-2 space-y-1 pl-4", children: section.bullets.map((bullet) => (_jsx("li", { className: "list-disc text-3 leading-6 text-ink-2", children: bullet }, bullet))) })) : null] }));
}
function GridPreview({ doc }) {
    const [cell, setCell] = useState({ row: 0, col: 1 });
    const rows = Array.from({ length: 8 }, (_, index) => ({
        name: doc.name.replace(/\.[^.]+$/, '') + ' · 行 ' + (index + 1),
        value: String(12 + index * 7),
        note: index % 3 === 0 ? '样本' : '—',
    }));
    const columns = ['项目', '数值', '备注'];
    const valueAt = (row, col) => {
        const record = rows[row];
        return col === 0 ? record.name : col === 1 ? record.value : record.note;
    };
    const cellName = String.fromCharCode(65 + cell.col) + (cell.row + 1);
    return (_jsxs("div", { className: "mt-3 overflow-hidden rounded-card border border-line", children: [_jsxs("div", { className: "flex items-center gap-1 border-b border-line bg-pane px-1.5 py-1 text-1", children: [_jsx("span", { className: "rounded-badge bg-hover px-2 py-[1px] text-ink", children: "Sheet1" }), _jsx("span", { className: "rounded-badge px-2 py-[1px] text-ink-3", children: "\u8BF4\u660E" }), _jsx("span", { className: "ml-auto text-ink-3", children: "\u539F\u578B\uFF1A\u53EA\u505A Sheet1" })] }), _jsxs("div", { className: "flex items-center gap-2 border-b border-line bg-content px-2 py-1 text-1", children: [_jsx("span", { className: "w-8 shrink-0 rounded-badge border border-line px-1 text-center brand-type text-ink-2", children: cellName }), _jsx("span", { className: "brand-type text-ink-3", children: "fx" }), _jsx("span", { className: "truncate text-ink-2", children: valueAt(cell.row, cell.col) })] }), _jsx("div", { className: "grid grid-cols-[2fr_1fr_1fr] bg-pane text-1 text-ink-3", children: columns.map((label, col) => (_jsxs("button", { type: "button", onClick: () => setCell((previous) => ({ ...previous, col })), className: "border-b border-line px-2 py-1.5 text-left transition-colors hover:text-ink-2", children: [String.fromCharCode(65 + col), " \u00B7 ", label] }, label))) }), rows.map((row, rowIndex) => (_jsx("div", { className: "grid grid-cols-[2fr_1fr_1fr] border-b border-line/60 text-2 last:border-0", children: columns.map((_label, col) => {
                    const active = cell.row === rowIndex && cell.col === col;
                    return (_jsx("button", { type: "button", onClick: () => setCell({ row: rowIndex, col }), className: cn('truncate px-2 py-1.5 text-left transition-colors', col === 1 ? 'brand-type text-ink' : 'text-ink-2', active ? 'bg-hover ring-1 ring-inset ring-ink-3/70' : 'hover:bg-hover/60'), children: col === 0 ? row.name : col === 1 ? row.value : row.note }, col));
                }) }, row.name)))] }));
}
function DocPreview({ doc }) {
    if (doc.layout === 'grid')
        return _jsx(GridPreview, { doc: doc });
    if (doc.layout === 'image') {
        return (_jsxs("div", { className: "mt-3", children: [_jsx("div", { className: "grid h-[260px] place-items-center rounded-card border border-line text-2 text-white/90", style: { background: 'linear-gradient(135deg, #d08a5a 0%, #f3c9a6 60%, #8b6cff 100%)' }, children: "\u56FE\u7247\u539F\u4EF6\uFF08\u539F\u578B\u7528\u8272\u5757\u5360\u4F4D\uFF09" }), _jsx("p", { className: "mt-2 text-1 text-ink-3", children: doc.capturedAt ? '抓取于 ' + doc.capturedAt : '原图按实际尺寸显示。' })] }));
    }
    if (doc.layout === 'snapshot') {
        return (_jsxs("div", { className: "mt-3 overflow-hidden rounded-card border border-line", children: [_jsxs("div", { className: "flex items-center gap-2 border-b border-line bg-pane px-2.5 py-1.5 text-1 text-ink-3", children: [_jsxs("span", { className: "flex gap-1", children: [_jsx("i", { className: "size-2 rounded-full bg-fail/60" }), _jsx("i", { className: "size-2 rounded-full bg-wait/60" }), _jsx("i", { className: "size-2 rounded-full bg-done/60" })] }), _jsx("span", { className: "truncate", children: doc.name }), doc.capturedAt ? _jsx("span", { className: "ml-auto shrink-0", children: doc.capturedAt }) : null] }), _jsxs("div", { className: "bg-raised/60 p-4", children: [_jsx("div", { className: "h-3 w-1/2 rounded bg-line-2" }), _jsx("div", { className: "mt-2 h-2 w-4/5 rounded bg-line" }), _jsx("div", { className: "mt-1.5 h-2 w-3/4 rounded bg-line" }), _jsx("div", { className: "mt-4 h-20 rounded bg-line/60" }), _jsx("p", { className: "mt-3 text-1 text-ink-3", children: "\u7F51\u9875\u5FEB\u7167\uFF08\u539F\u578B\u7528\u9AA8\u67B6\u5360\u4F4D\uFF1B\u771F\u5B9E\u5B9E\u73B0\u7528\u4FDD\u771F\u6E32\u67D3\uFF09" })] })] }));
    }
    if (doc.layout === 'code') {
        return (_jsx("pre", { className: "mt-3 overflow-x-auto rounded-card border border-line bg-pane p-3 font-mono text-2 leading-5 text-ink-2", children: doc.sections.map((section) => section.heading + String.fromCharCode(10) + section.paragraphs.join(String.fromCharCode(10))).join(String.fromCharCode(10, 10)) }));
    }
    return (_jsxs("div", { className: cn('mt-3', doc.layout === 'pages' ? 'rounded-card border border-line bg-raised/80 px-6 py-6 shadow-[var(--shadow-pop)]' : ''), children: [_jsx("h2", { className: "text-5 font-semibold leading-7 text-ink", children: doc.name.replace(/\.[^.]+$/, '') }), doc.sections.map((section) => (_jsx(SectionBody, { section: section }, section.heading)))] }));
}
function ReadMainPane() {
    const fileName = useRouteStore((state) => state.route.id);
    const reset = useReadTab((state) => state.reset);
    const [doc, setDoc] = useState(undefined);
    const [comparing, setComparing] = useState(false);
    const [editing, setEditing] = useState(false);
    const [draft, setDraft] = useState('');
    useEffect(() => {
        reset();
        setEditing(false);
        setComparing(false);
        let alive = true;
        setDoc(undefined);
        void (async () => {
            const next = await getRead(fileName);
            if (alive)
                setDoc(next);
        })();
        return () => {
            alive = false;
        };
    }, [fileName, reset]);
    if (doc === undefined)
        return _jsx("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: "\u6B63\u5728\u53D6\u6587\u4EF6\u2026" });
    if (!doc)
        return _jsxs("div", { className: "grid h-full place-items-center text-2 text-ink-3", children: ["\u6CA1\u6709\u8FD9\u4E2A\u6587\u4EF6\uFF1A", fileName] });
    return (_jsx("div", { className: "flex h-full min-h-0 flex-col overflow-hidden bg-content", "data-scene-page": "read", children: _jsx("div", { className: "min-h-0 flex-1 overflow-y-auto", children: _jsxs("div", { className: "mx-auto w-full max-w-[980px] px-5 py-4", children: [_jsxs("header", { className: "flex flex-wrap items-center gap-2", children: [_jsx("span", { className: "grid size-8 place-items-center rounded-info brand-type text-1 font-semibold text-white", style: { backgroundColor: TYPE_COLOR[doc.type] ?? 'var(--ink-3)' }, children: typeLabel(doc.type) }), _jsxs("div", { className: "min-w-0", children: [_jsx("h1", { className: "truncate text-4 font-medium text-ink", children: doc.name }), _jsxs("div", { className: "mt-0.5 flex flex-wrap items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { children: doc.origin === 'artifact' ? '成果' : '资料' }), _jsx("span", { children: "\u00B7" }), _jsx("span", { className: "truncate", children: doc.source }), doc.projectCode ? (_jsx("span", { className: "grid size-[14px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white", style: { backgroundColor: doc.projectColor ?? 'var(--ink-3)' }, children: doc.projectCode })) : null] })] }), _jsxs("div", { className: "ml-auto flex items-center gap-1.5", children: [doc.origin === 'material' ? (_jsx("button", { type: "button", onClick: () => {
                                            if (!editing) {
                                                setDraft(doc.sections.map((section) => (section.heading ? section.heading + String.fromCharCode(10) : '') + section.paragraphs.join(String.fromCharCode(10)) + (section.bullets?.length ? String.fromCharCode(10) + section.bullets.map((bullet) => '· ' + bullet).join(String.fromCharCode(10)) : '')).join(String.fromCharCode(10, 10)));
                                            }
                                            setEditing((value) => !value);
                                        }, className: cn('rounded-btn border px-2.5 py-1 text-2 transition-colors', editing ? 'border-ink-3 bg-hover text-ink' : 'border-line-2 text-ink-2 hover:bg-hover'), children: editing ? '取消编辑' : '编辑（一起写）' })) : null, doc.origin === 'artifact' ? (_jsx("button", { type: "button", disabled: !doc.diff, onClick: () => setComparing((value) => !value), className: cn('rounded-btn border px-2.5 py-1 text-2 transition-colors', doc.diff
                                            ? comparing
                                                ? 'border-ink-3 bg-hover text-ink'
                                                : 'border-line-2 text-ink-2 hover:bg-hover'
                                            : 'cursor-not-allowed border-line text-ink-3 opacity-60'), title: doc.diff ? '和上一版并排对比' : '这份文件没有上一版可对比', children: comparing ? '关闭对比' : '对比上一版' })) : null, _jsxs("button", { type: "button", onClick: () => toast('（原型）批注点选排在第 3 期；这里只给入口'), className: "flex items-center gap-1 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: [_jsx(MapPin, { className: "size-3" }), "\u6307\u4E00\u4E0B"] }), _jsx("button", { type: "button", onClick: () => toast('（原型）会用系统默认程序打开原件'), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u6253\u5F00\u539F\u4EF6" }), _jsx("button", { type: "button", onClick: () => toast('（原型）真实应用里会在 Finder 里显示'), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u5728 Finder \u4E2D\u663E\u793A" })] })] }), _jsxs("div", { className: "mt-2 flex flex-wrap items-center gap-1.5 text-1 text-ink-3", children: [_jsxs("span", { className: "rounded-btn border border-line-2 px-2 py-[1px] text-ink-2", children: [doc.version, "\uFF08\u5F53\u524D\uFF09"] }), doc.capturedAt ? _jsxs("span", { children: ["\u00B7 ", doc.capturedAt] }) : null, doc.fromTask ? (_jsxs("button", { type: "button", onClick: () => go('task:' + (doc.fromTask ?? '')), className: "ml-auto text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2", children: ["\u6765\u81EA\u300C", doc.fromTask, "\u300D\u203A"] })) : null] }), _jsx(Banner, { doc: doc }), _jsxs("div", { className: "mt-3 flex items-start gap-4", children: [_jsxs("aside", { className: "w-[150px] shrink-0", children: [_jsx("div", { className: "text-1 text-ink-3", children: "\u76EE\u5F55" }), _jsx("ul", { className: "mt-1 space-y-1", children: doc.layout === 'grid' ? (_jsx("li", { className: "text-2 text-ink-2", children: "Sheet1" })) : (doc.sections
                                            .filter((section) => section.heading)
                                            .map((section) => (_jsx("li", { className: "truncate text-2 text-ink-2", title: section.heading, children: section.heading }, section.heading)))) })] }), _jsx("div", { className: "min-w-0 flex-1", children: editing ? (_jsxs("div", { className: "mt-3", "data-read-editor": "true", children: [_jsxs("div", { className: "mb-1.5 flex items-center gap-2 text-1 text-ink-3", children: [_jsx("span", { children: "\u4E00\u8D77\u5199 \u00B7 \u8D44\u6599\u6539\u7684\u662F\u4FEE\u6539\u7A3F\uFF0C\u539F\u4EF6\u4E0D\u52A8" }), _jsx("span", { className: "ml-auto", children: "\u539F\u578B\uFF1A\u4FDD\u5B58\u53EA\u7ED9\u63D0\u793A" })] }), _jsx("textarea", { value: draft, onChange: (event) => setDraft(event.target.value), rows: 18, className: "w-full resize-y rounded-card border border-line bg-raised/70 p-4 font-mono text-2 leading-5 text-ink outline-none focus:border-ink-3" }), _jsxs("div", { className: "mt-2 flex items-center gap-2", children: [_jsx("button", { type: "button", onClick: () => {
                                                        setEditing(false);
                                                        toast('（原型）已存为修改稿；原件不动，修改稿等你验收');
                                                    }, className: "rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90", children: "\u5B58\u4E3A\u4FEE\u6539\u7A3F" }), _jsx("button", { type: "button", onClick: () => setEditing(false), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u53D6\u6D88" }), _jsx("span", { className: "text-1 text-ink-3", children: "\u5B58\u6210\u4FEE\u6539\u7A3F\u4EE5\u540E\uFF0C\u8C01\u5F15\u7528\u8FD9\u4EFD\u8D44\u6599\u4F1A\u770B\u5230\u300C\u6709\u65B0\u4FEE\u6539\u7A3F\u300D\u3002" })] })] })) : comparing && doc.diff ? (_jsx(DiffViewer, { doc: doc, onClose: () => setComparing(false) })) : (_jsx(DocPreview, { doc: doc })) })] }), _jsx("p", { className: "mt-6 border-t border-line pt-2 text-1 leading-4 text-ink-3", children: "AIOS \u6982\u5FF5 \u00B7 \u9605\u8BFB\u9875\uFF1A\u8D44\u6599\u548C\u6210\u679C\u5171\u7528\u540C\u4E00\u9875\uFF1B\u53F3\u680F\u4F34\u8BFB\u6536\u8D77\u540E\u6362\u6210\u9A8C\u6536\u9762\u677F\uFF0C\u9A8C\u6536\u6C38\u8FDC\u5BF9\u5E94\u5177\u4F53\u4E00\u7248\u3002" })] }) }) }));
}
function ReadSidePane() {
    const fileName = useRouteStore((state) => state.route.id);
    const tab = useReadTab((state) => state.tab);
    const setTab = useReadTab((state) => state.setTab);
    const [doc, setDoc] = useState(undefined);
    const [verdicts, setVerdicts] = useState({});
    const [accepted, setAccepted] = useState(false);
    useEffect(() => {
        let alive = true;
        setDoc(undefined);
        void (async () => {
            const next = await getRead(fileName);
            if (alive)
                setDoc(next);
        })();
        return () => {
            alive = false;
        };
    }, [fileName]);
    if (!doc) {
        return (_jsx("div", { className: "flex h-full items-start bg-content px-4 py-3 text-2 text-ink-3", "data-pane": "read-side", children: doc === undefined ? '正在取…' : '没有这个文件。' }));
    }
    const matched = doc.criteria.filter((criterion) => verdicts[criterion] !== undefined).length;
    const against = doc.criteria.filter((criterion) => verdicts[criterion] === false).length;
    return (_jsxs("div", { className: "flex h-full min-h-0 flex-col overflow-hidden bg-content", "data-pane": "read-side", children: [_jsxs("header", { className: "flex h-10 shrink-0 items-center gap-1 border-b border-line px-2", children: [['companion', 'notes', 'info'].map((key) => (_jsx("button", { type: "button", onClick: () => setTab(key), className: cn('rounded-btn px-2 py-1 text-2 transition-colors', tab === key ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink-2'), children: key === 'companion' ? '伴读' : key === 'notes' ? '批注' : '信息' }, key))), tab === 'accept' ? (_jsxs("span", { className: "flex items-center gap-1 rounded-btn bg-hover px-2 py-1 text-2 text-ink", children: [_jsx("button", { type: "button", onClick: () => setTab('companion'), "aria-label": "\u56DE\u5230\u4F34\u8BFB", children: _jsx(ArrowLeft, { className: "size-3.5" }) }), "\u9A8C\u6536"] })) : null] }), _jsxs("div", { className: "min-h-0 flex-1 overflow-y-auto px-3 py-3", children: [tab === 'companion' ? (_jsxs("div", { className: "space-y-3 text-2 leading-5", children: [_jsxs("div", { className: "max-w-[95%] text-ink-2", children: [_jsx("span", { className: "text-1 text-ink-3", children: "Kind" }), _jsx("p", { className: "mt-0.5", children: "\u6211\u5728\u65C1\u8FB9\u3002\u70B9\u300C\u6307\u4E00\u4E0B\u300D\u5B9A\u4E00\u6BB5\u539F\u6587\uFF0C\u6211\u5C31\u80FD\u63A5\u7740\u770B\uFF1B\u4E5F\u53EF\u4EE5\u76F4\u63A5\u5728\u4E0B\u9762\u95EE\u6211\u3002" })] }), _jsxs("div", { className: "max-w-[95%] rounded-card bg-pane/60 px-2.5 py-2 text-ink-2", children: [_jsx("div", { className: "text-1 text-ink-3", children: "Kind \u80FD\u770B\u5230" }), _jsxs("div", { className: "mt-1 text-2", children: [doc.name, " ", _jsx("span", { className: "brand-type text-ink-3", children: doc.version }), " \u00B7 \u548C Kind \u5168\u9875\u662F\u540C\u4E00\u6BB5\u5BF9\u8BDD\uFF08\u539F\u578B\u91CC\u53EA\u505A\u6837\u5B50\uFF09"] })] })] })) : null, tab === 'notes' ? (_jsxs("div", { className: "text-2 leading-5 text-ink-3", children: [_jsx("p", { children: "\u8FD8\u6CA1\u6709\u6279\u6CE8\u3002\u9009\u4E2D\u4E00\u6BB5\u6587\u5B57\u53EF\u4EE5\u6307\u7ED9\u6211\u2014\u2014\u6279\u6CE8\u70B9\u9009\u6392\u5728\u7B2C 3 \u671F\u3002" }), _jsx("p", { className: "mt-2", children: "\u9A8C\u6536\u65F6\u63D0\u7684\u4FEE\u6539\u8BB0\u5728\u300C\u9A8C\u6536\u610F\u89C1\u300D\u91CC\uFF0C\u4E0D\u5728\u6279\u6CE8\u91CC\u3002" })] })) : null, tab === 'info' ? (_jsxs("dl", { className: "space-y-2 text-2", children: [_jsxs("div", { children: [_jsx("dt", { className: "text-1 text-ink-3", children: "\u6765\u6E90" }), _jsx("dd", { className: "mt-0.5 text-ink-2", children: doc.source })] }), _jsxs("div", { children: [_jsx("dt", { className: "text-1 text-ink-3", children: "\u7248\u672C" }), _jsxs("dd", { className: "mt-0.5 text-ink-2", children: [_jsx("span", { className: "brand-type", children: doc.version }), "\uFF08\u5F53\u524D\uFF09"] })] }), _jsxs("div", { children: [_jsx("dt", { className: "text-1 text-ink-3", children: "\u88AB\u8C01\u7528\u8FC7" }), _jsx("dd", { className: "mt-0.5 space-y-0.5", children: doc.usedBy.length ? (doc.usedBy.map((row) => (_jsxs("div", { className: "text-ink-2", children: [row.task, " \u00B7 ", row.where] }, row.task + row.where)))) : (_jsx("span", { className: "text-ink-3", children: "\u8FD8\u6CA1\u6709\u3002" })) })] }), doc.status === 'acc' ? (_jsx("button", { type: "button", onClick: () => toast('（原型）已收进资料；别的项目和角色也能借用'), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u6536\u8FDB\u8D44\u6599" })) : null] })) : null, tab === 'accept' ? (accepted ? (_jsxs("div", { className: "text-2 leading-5 text-ink-2", children: [_jsxs("div", { className: "text-3 text-ink", children: ["\u4F60\u63A5\u53D7\u4E86 ", doc.version] }), _jsx("p", { className: "mt-1 text-1 text-ink-3", children: "\u8BB0\u5F55\u5728\u9A8C\u6536\u91CC\uFF1B\u4E4B\u540E\u518D\u6709\u65B0\u7248\u672C\u8981\u91CD\u65B0\u63A5\u53D7\u3002" }), _jsx("p", { className: "mt-2", children: "\u63A5\u4E0B\u6765\uFF1A\u6210\u679C\u6807\u6210 \u2713\u2713 \u5DF2\u63A5\u53D7\uFF1B\u9A8C\u6536\u90A3\u4E00\u7AD9\u5B8C\u6210\uFF1B\u53EF\u4EE5\u300C\u6536\u8FDB\u8D44\u6599\u300D\u3002" })] })) : (_jsxs("div", { className: "text-2", children: [_jsxs("div", { className: "grid grid-cols-2 gap-2", children: [_jsxs("div", { className: "rounded-card border border-line bg-pane/50 px-2.5 py-2", children: [_jsxs("div", { className: "flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx("i", { className: "size-1.5 rounded-full bg-done" }), "\u8981\u6C42\u5DF2\u6EE1\u8DB3"] }), _jsx("div", { className: "mt-1 text-2 text-ink", children: doc.criteria.length ? '逐条对照过' : '—' })] }), _jsxs("div", { className: "rounded-card border border-line bg-pane/50 px-2.5 py-2", children: [_jsxs("div", { className: "flex items-center gap-1.5 text-1 text-ink-3", children: [_jsx("span", { className: cn('text-1', accepted ? 'text-done' : 'text-ink-3'), children: accepted ? '✓✓' : '○' }), "\u4F60\u63A5\u53D7"] }), _jsx("div", { className: "mt-1 text-2 text-ink", children: accepted ? '已接受' : '还没' })] })] }), _jsx("p", { className: "mt-2 text-1 leading-4 text-ink-3", children: "\u8FD9\u662F\u4E24\u4EF6\u4E8B\uFF0C\u5206\u5F00\u8BB0\u3002\u4F60\u63A5\u53D7\u4E0D\u6539\u4EFB\u52A1\u8981\u6C42\uFF0C\u4E5F\u4E0D\u4EE3\u8868\u53D1\u5E03\uFF1B\u9010\u6761\u6253\u52FE\u4E0D\u4F1A\u66FF\u4F60\u63A5\u53D7\u3002" }), _jsx("div", { className: "mt-3 space-y-2", children: doc.criteria.length ? (doc.criteria.map((criterion) => (_jsxs("div", { className: "rounded-card border border-line bg-pane/40 p-2.5", children: [_jsx("div", { className: "text-2 text-ink", children: criterion }), _jsxs("div", { className: "mt-1 flex items-center gap-2 text-1 text-ink-3", children: [_jsx("span", { children: "\u2699 \u673A\u5668\u9A8C\u8BC1" }), _jsx("span", { children: "\u00B7" }), _jsx("span", { children: "\u5BF9\u7167\u539F\u6587\u5224\u65AD" })] }), _jsxs("div", { className: "mt-1.5 flex gap-1.5", children: [_jsx("button", { type: "button", onClick: () => setVerdicts((prev) => ({ ...prev, [criterion]: true })), className: cn('rounded-btn border px-2 py-[2px] text-1 transition-colors', verdicts[criterion] === true
                                                        ? 'border-done bg-done-soft text-done'
                                                        : 'border-line-2 text-ink-3 hover:bg-hover'), children: "\u2713 \u7B26\u5408" }), _jsx("button", { type: "button", onClick: () => setVerdicts((prev) => ({ ...prev, [criterion]: false })), className: cn('rounded-btn border px-2 py-[2px] text-1 transition-colors', verdicts[criterion] === false
                                                        ? 'border-fail bg-fail-soft text-fail'
                                                        : 'border-line-2 text-ink-3 hover:bg-hover'), children: "\u2717 \u4E0D\u7B26\u5408" })] })] }, criterion)))) : (_jsx("p", { className: "text-2 text-ink-3", children: "\u8FD9\u4EFD\u6587\u4EF6\u6CA1\u6709\u5217\u5B8C\u6210\u6807\u51C6\u3002" })) }), _jsxs("p", { className: "mt-2 text-1 text-ink-3", children: ["\u4F60\u770B\u4E86 ", _jsx("span", { className: "brand-type", children: matched }), " / ", doc.criteria.length, " \u6761", against ? ' · ' + against + ' 条不符合' : ''] }), _jsx("div", { className: "mt-2 flex flex-wrap gap-2", children: against ? (_jsxs(_Fragment, { children: [_jsxs("button", { type: "button", onClick: () => toast('（原型）提了 ' + against + ' 条修改；会开一轮返工'), className: "rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90", children: ["\u63D0\u4FEE\u6539\uFF08", against, " \u6761\uFF09"] }), _jsxs("button", { type: "button", onClick: () => {
                                                setAccepted(true);
                                                toast('（原型）已接受 ' + doc.version + '（记录里注明：接受时标了 ' + against + ' 条不符合）');
                                            }, className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: ["\u4ECD\u7136\u63A5\u53D7 ", doc.version] })] })) : (_jsxs(_Fragment, { children: [_jsxs("button", { type: "button", onClick: () => {
                                                setAccepted(true);
                                                toast('（原型）已接受 ' + doc.version);
                                            }, className: "rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90", children: ["\u63A5\u53D7 ", doc.version] }), _jsx("button", { type: "button", onClick: () => toast('（原型）提修改：会在任务页开一轮返工'), className: "rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover", children: "\u63D0\u4FEE\u6539" })] })) })] }))) : null] }), _jsx("div", { className: "shrink-0 border-t border-line px-3 py-2", children: _jsxs("div", { className: "flex items-end gap-1.5 rounded-btn border border-line-2 bg-pane px-2 py-1.5", children: [_jsx("textarea", { rows: 1, placeholder: "\u95EE Kind\uFF0C\u6216\u8BF4\u8BF4\u4F60\u5BF9\u8FD9\u6BB5\u7684\u770B\u6CD5\u2026", className: "max-h-24 min-h-5 flex-1 resize-none bg-transparent text-2 text-ink outline-none placeholder:text-ink-3" }), _jsxs("button", { type: "button", onClick: () => toast('（原型）批注点选排在第 3 期；现在先在对话里说'), className: "flex h-6 items-center gap-1 rounded-btn px-1.5 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2", children: [_jsx(MapPin, { className: "size-3" }), "\u6307\u4E00\u4E0B"] }), _jsx("button", { type: "button", onClick: () => toast('（原型）阅读页的对话只做样子'), className: "grid size-6 shrink-0 place-items-center rounded-full bg-hover text-ink-3 transition-colors hover:text-ink-2", children: _jsx(SendHorizontal, { className: "size-3.5" }) })] }) })] }));
}
export function ReadScene({ pane }) {
    if (pane === 'side')
        return _jsx(ReadSidePane, {});
    return _jsx(ReadMainPane, {});
}
