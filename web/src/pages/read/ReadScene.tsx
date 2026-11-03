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
import type { ReadDoc, ReadSection } from '@/data/selectors';
import { go } from '@/app/navigate';
import { useRouteStore } from '@/app/routeStore';
import { cn } from '@/lib/utils';

type ReadTab = 'companion' | 'notes' | 'info' | 'accept';

/** 阅读页右栏的页签：横幅上的「开始验收」也要能把右栏切过去，所以放一个小 store（按文件重置）。 */
const useReadTab = create<{ tab: ReadTab; setTab: (tab: ReadTab) => void; reset: () => void }>()((set) => ({
  tab: 'companion',
  setTab: (tab) => set({ tab }),
  reset: () => set({ tab: 'companion' }),
}));

/** v13-o O2 的 diff 查看器：并排列号 + 增删改颜色（用状态色，不另发明颜色）。 */
function DiffViewer({ doc, onClose }: { doc: ReadDoc; onClose: () => void }) {
  if (!doc.diff) return null;
  const tone: Record<string, string> = {
    same: 'bg-transparent',
    chg: 'bg-wait-soft',
    add: 'bg-done-soft',
    del: 'bg-fail-soft',
  };
  return (
    <div className="mt-3 overflow-hidden rounded-card border border-line">
      <div className="flex items-center gap-2 border-b border-line bg-pane px-2.5 py-1.5 text-2">
        <span className="text-ink-2">对比上一版</span>
        <span className="text-1 text-ink-3">
          v1 → {doc.version} · 改动处用颜色标出来（原型里是写好的样例）
        </span>
        <button
          type="button"
          onClick={onClose}
          className="ml-auto rounded-btn border border-line-2 px-2 py-[2px] text-1 text-ink-2 transition-colors hover:bg-hover"
        >
          关闭对比
        </button>
      </div>
      <div className="grid grid-cols-2 divide-x divide-line text-1 leading-5">
        {(['left', 'right'] as const).map((side) => (
          <div key={side} className="min-w-0">
            <div className="border-b border-line bg-pane px-2 py-1 text-ink-3">{side === 'left' ? 'v1（上一版）' : doc.version + '（当前）'}</div>
            {doc.diff![side].map((line, index) => (
              <div key={side + index} className={cn('flex gap-2 px-2 py-[2px] font-mono text-2', tone[doc.diff!.status[index] ?? 'same'])}>
                <span className="w-5 shrink-0 text-right brand-type text-ink-3">{index + 1}</span>
                <span className="min-w-0 break-all text-ink-2">{line}</span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

const TYPE_COLOR: Record<string, string> = {
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

function typeLabel(type: string): string {
  return (type || 'file').toUpperCase();
}

function Banner({ doc }: { doc: ReadDoc }) {
  const setTab = useReadTab((state) => state.setTab);
  if (doc.status === 'pend') {
    return (
      <div className="mt-3 flex flex-wrap items-center gap-2 rounded-card border border-wait/40 bg-wait-soft/50 px-3 py-2 text-2 text-wait">
        <span>这份成果等你验收 · 完成标准 {doc.criteria.length} 条</span>
        <button
          type="button"
          onClick={() => setTab('accept')}
          className="ml-auto rounded-btn bg-wait px-2.5 py-1 text-1 font-semibold text-content"
        >
          开始验收
        </button>
      </div>
    );
  }
  if (doc.status === 'acc') {
    return (
      <div className="mt-3 rounded-card border border-done/40 bg-done-soft/50 px-3 py-2 text-2 text-done">
        ✓✓ 已接受 {doc.version} · 记录在验收面板里；之后再有新版本要重新接受。
      </div>
    );
  }
  if (doc.status === 'bad') {
    return (
      <div className="mt-3 rounded-card border border-fail/40 bg-fail-soft/50 px-3 py-2 text-2 text-fail">
        打不开 · 现在显示的是旧快照，不再当作最新内容。
      </div>
    );
  }
  if (doc.status === 'busy') {
    return (
      <div className="mt-3 rounded-card border border-wait/40 bg-wait-soft/50 px-3 py-2 text-2 text-wait">
        正在整理 · 现在能读，但还搜不全。
      </div>
    );
  }
  if (doc.origin === 'artifact') {
    return <div className="mt-3 rounded-card border border-line bg-pane/60 px-3 py-2 text-2 text-ink-3">草稿 · 还没提交验收。</div>;
  }
  return null;
}

function SectionBody({ section }: { section: ReadSection }) {
  return (
    <div className="mt-5 first:mt-0">
      {section.heading ? <h3 className="text-4 font-medium leading-6 text-ink">{section.heading}</h3> : null}
      {section.paragraphs.map((paragraph) => (
        <p key={paragraph} className="mt-2 text-3 leading-6 text-ink-2">
          {paragraph}
        </p>
      ))}
      {section.bullets?.length ? (
        <ul className="mt-2 space-y-1 pl-4">
          {section.bullets.map((bullet) => (
            <li key={bullet} className="list-disc text-3 leading-6 text-ink-2">
              {bullet}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function GridPreview({ doc }: { doc: ReadDoc }) {
  const [cell, setCell] = useState<{ row: number; col: number }>({ row: 0, col: 1 });
  const rows = Array.from({ length: 8 }, (_, index) => ({
    name: doc.name.replace(/\.[^.]+$/, '') + ' · 行 ' + (index + 1),
    value: String(12 + index * 7),
    note: index % 3 === 0 ? '样本' : '—',
  }));
  const columns = ['项目', '数值', '备注'];
  const valueAt = (row: number, col: number): string => {
    const record = rows[row];
    return col === 0 ? record.name : col === 1 ? record.value : record.note;
  };
  const cellName = String.fromCharCode(65 + cell.col) + (cell.row + 1);
  return (
    <div className="mt-3 overflow-hidden rounded-card border border-line">
      {/* 工作表页签（v13-o O2：表格查看器要有页签） */}
      <div className="flex items-center gap-1 border-b border-line bg-pane px-1.5 py-1 text-1">
        <span className="rounded-badge bg-hover px-2 py-[1px] text-ink">Sheet1</span>
        <span className="rounded-badge px-2 py-[1px] text-ink-3">说明</span>
        <span className="ml-auto text-ink-3">原型：只做 Sheet1</span>
      </div>
      {/* 公式栏（点哪个格就显示哪个格的值） */}
      <div className="flex items-center gap-2 border-b border-line bg-content px-2 py-1 text-1">
        <span className="w-8 shrink-0 rounded-badge border border-line px-1 text-center brand-type text-ink-2">{cellName}</span>
        <span className="brand-type text-ink-3">fx</span>
        <span className="truncate text-ink-2">{valueAt(cell.row, cell.col)}</span>
      </div>
      {/* 冻结表头 + 网格，数字用 Overpass */}
      <div className="grid grid-cols-[2fr_1fr_1fr] bg-pane text-1 text-ink-3">
        {columns.map((label, col) => (
          <button
            key={label}
            type="button"
            onClick={() => setCell((previous) => ({ ...previous, col }))}
            className="border-b border-line px-2 py-1.5 text-left transition-colors hover:text-ink-2"
          >
            {String.fromCharCode(65 + col)} · {label}
          </button>
        ))}
      </div>
      {rows.map((row, rowIndex) => (
        <div key={row.name} className="grid grid-cols-[2fr_1fr_1fr] border-b border-line/60 text-2 last:border-0">
          {columns.map((_label, col) => {
            const active = cell.row === rowIndex && cell.col === col;
            return (
              <button
                key={col}
                type="button"
                onClick={() => setCell({ row: rowIndex, col })}
                className={cn(
                  'truncate px-2 py-1.5 text-left transition-colors',
                  col === 1 ? 'brand-type text-ink' : 'text-ink-2',
                  active ? 'bg-hover ring-1 ring-inset ring-ink-3/70' : 'hover:bg-hover/60',
                )}
              >
                {col === 0 ? row.name : col === 1 ? row.value : row.note}
              </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}

function DocPreview({ doc }: { doc: ReadDoc }) {
  if (doc.layout === 'grid') return <GridPreview doc={doc} />;
  if (doc.layout === 'image') {
    return (
      <div className="mt-3">
        <div
          className="grid h-[260px] place-items-center rounded-card border border-line text-2 text-white/90"
          style={{ background: 'linear-gradient(135deg, #d08a5a 0%, #f3c9a6 60%, #8b6cff 100%)' }}
        >
          图片原件（原型用色块占位）
        </div>
        <p className="mt-2 text-1 text-ink-3">{doc.capturedAt ? '抓取于 ' + doc.capturedAt : '原图按实际尺寸显示。'}</p>
      </div>
    );
  }
  if (doc.layout === 'snapshot') {
    return (
      <div className="mt-3 overflow-hidden rounded-card border border-line">
        <div className="flex items-center gap-2 border-b border-line bg-pane px-2.5 py-1.5 text-1 text-ink-3">
          <span className="flex gap-1">
            <i className="size-2 rounded-full bg-fail/60" />
            <i className="size-2 rounded-full bg-wait/60" />
            <i className="size-2 rounded-full bg-done/60" />
          </span>
          <span className="truncate">{doc.name}</span>
          {doc.capturedAt ? <span className="ml-auto shrink-0">{doc.capturedAt}</span> : null}
        </div>
        <div className="bg-raised/60 p-4">
          <div className="h-3 w-1/2 rounded bg-line-2" />
          <div className="mt-2 h-2 w-4/5 rounded bg-line" />
          <div className="mt-1.5 h-2 w-3/4 rounded bg-line" />
          <div className="mt-4 h-20 rounded bg-line/60" />
          <p className="mt-3 text-1 text-ink-3">网页快照（原型用骨架占位；真实实现用保真渲染）</p>
        </div>
      </div>
    );
  }
  if (doc.layout === 'code') {
    return (
      <pre className="mt-3 overflow-x-auto rounded-card border border-line bg-pane p-3 font-mono text-2 leading-5 text-ink-2">
        {doc.sections.map((section) => section.heading + String.fromCharCode(10) + section.paragraphs.join(String.fromCharCode(10))).join(String.fromCharCode(10, 10))}
      </pre>
    );
  }
  return (
    <div
      className={cn(
        'mt-3',
        doc.layout === 'pages' ? 'rounded-card border border-line bg-raised/80 px-6 py-6 shadow-[var(--shadow-pop)]' : '',
      )}
    >
      <h2 className="text-5 font-semibold leading-7 text-ink">{doc.name.replace(/\.[^.]+$/, '')}</h2>
      {doc.sections.map((section) => (
        <SectionBody key={section.heading} section={section} />
      ))}
    </div>
  );
}

function ReadMainPane() {
  const fileName = useRouteStore((state) => state.route.id);
  const reset = useReadTab((state) => state.reset);
  const [doc, setDoc] = useState<ReadDoc | null | undefined>(undefined);
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
      if (alive) setDoc(next);
    })();
    return () => {
      alive = false;
    };
  }, [fileName, reset]);

  if (doc === undefined) return <div className="grid h-full place-items-center text-2 text-ink-3">正在取文件…</div>;
  if (!doc) return <div className="grid h-full place-items-center text-2 text-ink-3">没有这个文件：{fileName}</div>;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-scene-page="read">
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[980px] px-5 py-4">
          <header className="flex flex-wrap items-center gap-2">
            <span
              className="grid size-8 place-items-center rounded-info brand-type text-1 font-semibold text-white"
              style={{ backgroundColor: TYPE_COLOR[doc.type] ?? 'var(--ink-3)' }}
            >
              {typeLabel(doc.type)}
            </span>
            <div className="min-w-0">
              <h1 className="truncate text-4 font-medium text-ink">{doc.name}</h1>
              <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-1 text-ink-3">
                <span>{doc.origin === 'artifact' ? '成果' : '资料'}</span>
                <span>·</span>
                <span className="truncate">{doc.source}</span>
                {doc.projectCode ? (
                  <span
                    className="grid size-[14px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white"
                    style={{ backgroundColor: doc.projectColor ?? 'var(--ink-3)' }}
                  >
                    {doc.projectCode}
                  </span>
                ) : null}
              </div>
            </div>
            <div className="ml-auto flex items-center gap-1.5">
              {doc.origin === 'material' ? (
                <button
                  type="button"
                  onClick={() => {
                    if (!editing) {
                      setDraft(doc.sections.map((section) => (section.heading ? section.heading + String.fromCharCode(10) : '') + section.paragraphs.join(String.fromCharCode(10)) + (section.bullets?.length ? String.fromCharCode(10) + section.bullets.map((bullet) => '· ' + bullet).join(String.fromCharCode(10)) : '')).join(String.fromCharCode(10, 10)));
                    }
                    setEditing((value) => !value);
                  }}
                  className={cn(
                    'rounded-btn border px-2.5 py-1 text-2 transition-colors',
                    editing ? 'border-ink-3 bg-hover text-ink' : 'border-line-2 text-ink-2 hover:bg-hover',
                  )}
                >
                  {editing ? '取消编辑' : '编辑（一起写）'}
                </button>
              ) : null}
              {doc.origin === 'artifact' ? (
                <button
                  type="button"
                  disabled={!doc.diff}
                  onClick={() => setComparing((value) => !value)}
                  className={cn(
                    'rounded-btn border px-2.5 py-1 text-2 transition-colors',
                    doc.diff
                      ? comparing
                        ? 'border-ink-3 bg-hover text-ink'
                        : 'border-line-2 text-ink-2 hover:bg-hover'
                      : 'cursor-not-allowed border-line text-ink-3 opacity-60',
                  )}
                  title={doc.diff ? '和上一版并排对比' : '这份文件没有上一版可对比'}
                >
                  {comparing ? '关闭对比' : '对比上一版'}
                </button>
              ) : null}
              <button
                type="button"
                onClick={() => toast('（原型）批注点选排在第 3 期；这里只给入口')}
                className="flex items-center gap-1 rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
              >
                <MapPin className="size-3" />
                指一下
              </button>
              <button
                type="button"
                onClick={() => toast('（原型）会用系统默认程序打开原件')}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
              >
                打开原件
              </button>
              <button
                type="button"
                onClick={() => toast('（原型）真实应用里会在 Finder 里显示')}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
              >
                在 Finder 中显示
              </button>
            </div>
          </header>

          <div className="mt-2 flex flex-wrap items-center gap-1.5 text-1 text-ink-3">
            <span className="rounded-btn border border-line-2 px-2 py-[1px] text-ink-2">{doc.version}（当前）</span>
            {doc.capturedAt ? <span>· {doc.capturedAt}</span> : null}
            {doc.fromTask ? (
              <button
                type="button"
                onClick={() => go('task:' + (doc.fromTask ?? ''))}
                className="ml-auto text-ink-3 underline decoration-dotted transition-colors hover:text-ink-2"
              >
                来自「{doc.fromTask}」›
              </button>
            ) : null}
          </div>

          <Banner doc={doc} />

          <div className="mt-3 flex items-start gap-4">
            <aside className="w-[150px] shrink-0">
              <div className="text-1 text-ink-3">目录</div>
              <ul className="mt-1 space-y-1">
                {doc.layout === 'grid' ? (
                  <li className="text-2 text-ink-2">Sheet1</li>
                ) : (
                  doc.sections
                    .filter((section) => section.heading)
                    .map((section) => (
                      <li key={section.heading} className="truncate text-2 text-ink-2" title={section.heading}>
                        {section.heading}
                      </li>
                    ))
                )}
              </ul>
            </aside>
            <div className="min-w-0 flex-1">
              {editing ? (
                <div className="mt-3" data-read-editor="true">
                  <div className="mb-1.5 flex items-center gap-2 text-1 text-ink-3">
                    <span>一起写 · 资料改的是修改稿，原件不动</span>
                    <span className="ml-auto">原型：保存只给提示</span>
                  </div>
                  <textarea
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                    rows={18}
                    className="w-full resize-y rounded-card border border-line bg-raised/70 p-4 font-mono text-2 leading-5 text-ink outline-none focus:border-ink-3"
                  />
                  <div className="mt-2 flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => {
                        setEditing(false);
                        toast('（原型）已存为修改稿；原件不动，修改稿等你验收');
                      }}
                      className="rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90"
                    >
                      存为修改稿
                    </button>
                    <button
                      type="button"
                      onClick={() => setEditing(false)}
                      className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
                    >
                      取消
                    </button>
                    <span className="text-1 text-ink-3">存成修改稿以后，谁引用这份资料会看到「有新修改稿」。</span>
                  </div>
                </div>
              ) : comparing && doc.diff ? (
                <DiffViewer doc={doc} onClose={() => setComparing(false)} />
              ) : (
                <DocPreview doc={doc} />
              )}
            </div>
          </div>

          <p className="mt-6 border-t border-line pt-2 text-1 leading-4 text-ink-3">
            AIOS 概念 · 阅读页：资料和成果共用同一页；右栏伴读收起后换成验收面板，验收永远对应具体一版。
          </p>
        </div>
      </div>
    </div>
  );
}

function ReadSidePane() {
  const fileName = useRouteStore((state) => state.route.id);
  const tab = useReadTab((state) => state.tab);
  const setTab = useReadTab((state) => state.setTab);
  const [doc, setDoc] = useState<ReadDoc | null | undefined>(undefined);
  const [verdicts, setVerdicts] = useState<Record<string, boolean>>({});
  const [accepted, setAccepted] = useState(false);

  useEffect(() => {
    let alive = true;
    setDoc(undefined);
    void (async () => {
      const next = await getRead(fileName);
      if (alive) setDoc(next);
    })();
    return () => {
      alive = false;
    };
  }, [fileName]);

  if (!doc) {
    return (
      <div className="flex h-full items-start bg-content px-4 py-3 text-2 text-ink-3" data-pane="read-side">
        {doc === undefined ? '正在取…' : '没有这个文件。'}
      </div>
    );
  }

  const matched = doc.criteria.filter((criterion) => verdicts[criterion] !== undefined).length;
  const against = doc.criteria.filter((criterion) => verdicts[criterion] === false).length;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-content" data-pane="read-side">
      <header className="flex h-10 shrink-0 items-center gap-1 border-b border-line px-2">
        {(['companion', 'notes', 'info'] as const).map((key) => (
          <button
            key={key}
            type="button"
            onClick={() => setTab(key)}
            className={cn(
              'rounded-btn px-2 py-1 text-2 transition-colors',
              tab === key ? 'bg-hover text-ink' : 'text-ink-3 hover:text-ink-2',
            )}
          >
            {key === 'companion' ? '伴读' : key === 'notes' ? '批注' : '信息'}
          </button>
        ))}
        {tab === 'accept' ? (
          <span className="flex items-center gap-1 rounded-btn bg-hover px-2 py-1 text-2 text-ink">
            <button type="button" onClick={() => setTab('companion')} aria-label="回到伴读">
              <ArrowLeft className="size-3.5" />
            </button>
            验收
          </span>
        ) : null}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {tab === 'companion' ? (
          <div className="space-y-3 text-2 leading-5">
            <div className="max-w-[95%] text-ink-2">
              <span className="text-1 text-ink-3">Kind</span>
              <p className="mt-0.5">我在旁边。点「指一下」定一段原文，我就能接着看；也可以直接在下面问我。</p>
            </div>
            <div className="max-w-[95%] rounded-card bg-pane/60 px-2.5 py-2 text-ink-2">
              <div className="text-1 text-ink-3">Kind 能看到</div>
              <div className="mt-1 text-2">
                {doc.name} <span className="brand-type text-ink-3">{doc.version}</span> · 和 Kind 全页是同一段对话（原型里只做样子）
              </div>
            </div>
          </div>
        ) : null}

        {tab === 'notes' ? (
          <div className="text-2 leading-5 text-ink-3">
            <p>还没有批注。选中一段文字可以指给我——批注点选排在第 3 期。</p>
            <p className="mt-2">验收时提的修改记在「验收意见」里，不在批注里。</p>
          </div>
        ) : null}

        {tab === 'info' ? (
          <dl className="space-y-2 text-2">
            <div>
              <dt className="text-1 text-ink-3">来源</dt>
              <dd className="mt-0.5 text-ink-2">{doc.source}</dd>
            </div>
            <div>
              <dt className="text-1 text-ink-3">版本</dt>
              <dd className="mt-0.5 text-ink-2">
                <span className="brand-type">{doc.version}</span>（当前）
              </dd>
            </div>
            <div>
              <dt className="text-1 text-ink-3">被谁用过</dt>
              <dd className="mt-0.5 space-y-0.5">
                {doc.usedBy.length ? (
                  doc.usedBy.map((row) => (
                    <div key={row.task + row.where} className="text-ink-2">
                      {row.task} · {row.where}
                    </div>
                  ))
                ) : (
                  <span className="text-ink-3">还没有。</span>
                )}
              </dd>
            </div>
            {doc.status === 'acc' ? (
              <button
                type="button"
                onClick={() => toast('（原型）已收进资料；别的项目和角色也能借用')}
                className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
              >
                收进资料
              </button>
            ) : null}
          </dl>
        ) : null}

        {tab === 'accept' ? (
          accepted ? (
            <div className="text-2 leading-5 text-ink-2">
              <div className="text-3 text-ink">你接受了 {doc.version}</div>
              <p className="mt-1 text-1 text-ink-3">记录在验收里；之后再有新版本要重新接受。</p>
              <p className="mt-2">接下来：成果标成 ✓✓ 已接受；验收那一站完成；可以「收进资料」。</p>
            </div>
          ) : (
            <div className="text-2">
              <div className="grid grid-cols-2 gap-2">
                <div className="rounded-card border border-line bg-pane/50 px-2.5 py-2">
                  <div className="flex items-center gap-1.5 text-1 text-ink-3">
                    <i className="size-1.5 rounded-full bg-done" />
                    要求已满足
                  </div>
                  <div className="mt-1 text-2 text-ink">{doc.criteria.length ? '逐条对照过' : '—'}</div>
                </div>
                <div className="rounded-card border border-line bg-pane/50 px-2.5 py-2">
                  <div className="flex items-center gap-1.5 text-1 text-ink-3">
                    <span className={cn('text-1', accepted ? 'text-done' : 'text-ink-3')}>{accepted ? '✓✓' : '○'}</span>
                    你接受
                  </div>
                  <div className="mt-1 text-2 text-ink">{accepted ? '已接受' : '还没'}</div>
                </div>
              </div>
              <p className="mt-2 text-1 leading-4 text-ink-3">这是两件事，分开记。你接受不改任务要求，也不代表发布；逐条打勾不会替你接受。</p>

              <div className="mt-3 space-y-2">
                {doc.criteria.length ? (
                  doc.criteria.map((criterion) => (
                    <div key={criterion} className="rounded-card border border-line bg-pane/40 p-2.5">
                      <div className="text-2 text-ink">{criterion}</div>
                      <div className="mt-1 flex items-center gap-2 text-1 text-ink-3">
                        <span>⚙ 机器验证</span>
                        <span>·</span>
                        <span>对照原文判断</span>
                      </div>
                      <div className="mt-1.5 flex gap-1.5">
                        <button
                          type="button"
                          onClick={() => setVerdicts((prev) => ({ ...prev, [criterion]: true }))}
                          className={cn(
                            'rounded-btn border px-2 py-[2px] text-1 transition-colors',
                            verdicts[criterion] === true
                              ? 'border-done bg-done-soft text-done'
                              : 'border-line-2 text-ink-3 hover:bg-hover',
                          )}
                        >
                          ✓ 符合
                        </button>
                        <button
                          type="button"
                          onClick={() => setVerdicts((prev) => ({ ...prev, [criterion]: false }))}
                          className={cn(
                            'rounded-btn border px-2 py-[2px] text-1 transition-colors',
                            verdicts[criterion] === false
                              ? 'border-fail bg-fail-soft text-fail'
                              : 'border-line-2 text-ink-3 hover:bg-hover',
                          )}
                        >
                          ✗ 不符合
                        </button>
                      </div>
                    </div>
                  ))
                ) : (
                  <p className="text-2 text-ink-3">这份文件没有列完成标准。</p>
                )}
              </div>

              <p className="mt-2 text-1 text-ink-3">
                你看了 <span className="brand-type">{matched}</span> / {doc.criteria.length} 条
                {against ? ' · ' + against + ' 条不符合' : ''}
              </p>
              <div className="mt-2 flex flex-wrap gap-2">
                {against ? (
                  <>
                    <button
                      type="button"
                      onClick={() => toast('（原型）提了 ' + against + ' 条修改；会开一轮返工')}
                      className="rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90"
                    >
                      提修改（{against} 条）
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setAccepted(true);
                        toast('（原型）已接受 ' + doc.version + '（记录里注明：接受时标了 ' + against + ' 条不符合）');
                      }}
                      className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
                    >
                      仍然接受 {doc.version}
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      type="button"
                      onClick={() => {
                        setAccepted(true);
                        toast('（原型）已接受 ' + doc.version);
                      }}
                      className="rounded-btn bg-ink px-2.5 py-1 text-2 text-content transition-opacity hover:opacity-90"
                    >
                      接受 {doc.version}
                    </button>
                    <button
                      type="button"
                      onClick={() => toast('（原型）提修改：会在任务页开一轮返工')}
                      className="rounded-btn border border-line-2 px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover"
                    >
                      提修改
                    </button>
                  </>
                )}
              </div>
            </div>
          )
        ) : null}
      </div>

      <div className="shrink-0 border-t border-line px-3 py-2">
        <div className="flex items-end gap-1.5 rounded-btn border border-line-2 bg-pane px-2 py-1.5">
          <textarea
            rows={1}
            placeholder="问 Kind，或说说你对这段的看法…"
            className="max-h-24 min-h-5 flex-1 resize-none bg-transparent text-2 text-ink outline-none placeholder:text-ink-3"
          />
          <button
            type="button"
            onClick={() => toast('（原型）批注点选排在第 3 期；现在先在对话里说')}
            className="flex h-6 items-center gap-1 rounded-btn px-1.5 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
          >
            <MapPin className="size-3" />
            指一下
          </button>
          <button
            type="button"
            onClick={() => toast('（原型）阅读页的对话只做样子')}
            className="grid size-6 shrink-0 place-items-center rounded-full bg-hover text-ink-3 transition-colors hover:text-ink-2"
          >
            <SendHorizontal className="size-3.5" />
          </button>
        </div>
      </div>
    </div>
  );
}

export function ReadScene({ pane }: { pane: string }) {
  if (pane === 'side') return <ReadSidePane />;
  return <ReadMainPane />;
}
