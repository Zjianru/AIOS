/**
 * 记忆页（10 篇 10.6 / 08 篇）：搜（记忆、日记、Kind 对话，带出处）、关于你、
 * 条目（左范围、右「你定的 / Kind 记的」，Kind 记的能定下来）、Kind 的日记（只读）、
 * 做梦（开关/间隔/上次/下次/马上整理/历次明细）、隔离区。
 *
 * 数据一次 MemoryRead 读全量；动作一次 MemoryAction；服务推 memory/changed 就重读。
 */

import { useEffect, useMemo, useState } from 'react';
import { Bookmark, Brain, RotateCcw, Search, Trash2 } from 'lucide-react';
import type { MemoryDreamRunView, MemoryItemView, MemoryRevisionView } from '@/data/service';
import { memoryRead } from '@/data/service';
import type { MemoryActionKind } from '@/data/service';
import { mountMemoryLive, useMemoryState } from './memoryStore';

function clock(at: number | null | undefined): string {
  if (!at) return '未知';
  const date = new Date(at);
  const pad = (value: number) => String(value).padStart(2, '0');
  return (
    String(date.getFullYear()) +
    '-' +
    pad(date.getMonth() + 1) +
    '-' +
    pad(date.getDate()) +
    ' ' +
    pad(date.getHours()) +
    ':' +
    pad(date.getMinutes())
  );
}

function dayOf(at: number): string {
  const date = new Date(at);
  const pad = (value: number) => String(value).padStart(2, '0');
  return pad(date.getMonth() + 1) + '-' + pad(date.getDate());
}

function Section({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-btn border border-line bg-content p-4">
      <div className="mb-3 flex items-baseline gap-2">
        <h2 className="text-3 font-medium text-ink">{title}</h2>
        {hint ? <span className="text-2 text-ink-3">{hint}</span> : null}
      </div>
      {children}
    </section>
  );
}

const actionClass =
  'shrink-0 text-2 text-ink-3 underline-offset-2 hover:text-ink hover:underline';
const primaryActionClass =
  'shrink-0 text-2 text-brand underline-offset-2 hover:underline';

function ItemRow({
  item,
  onEdit,
  onDelete,
  onPin,
}: {
  item: MemoryItemView;
  onEdit?: () => void;
  onDelete?: () => void;
  onPin?: () => void;
}) {
  return (
    <div
      data-kind="memory-item"
      data-id={item.id}
      data-layer={item.layer}
      data-scope={item.scope}
      data-binding={item.binding}
      className="flex items-start gap-2 border-b border-line-2 py-2 last:border-b-0"
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-3 text-ink">{item.title || '（无标题）'}</span>
          <span className="shrink-0 rounded-btn bg-hover px-1.5 text-2 text-ink-3">
            {item.scope === 'personal' ? '个人' : item.scope === 'project' ? '项目' : '角色'}
          </span>
          {item.status === 'pinned' ? (
            <span className="shrink-0 text-2 text-ink-3">已定下来</span>
          ) : null}
        </div>
        <p className="mt-0.5 whitespace-pre-wrap text-2 leading-5 text-ink-2">
          {item.body || item.summary || '（空）'}
        </p>
        <p className="mt-1 text-1 text-ink-3">
          来源 {item.sourceKind} · 第 {item.revision} 版 · {clock(item.updatedAt)}
        </p>
      </div>
      <div className="flex shrink-0 flex-col items-end gap-1">
        {onEdit ? (
          <button type="button" data-action="memory-correct" onClick={onEdit} className={actionClass}>
            改
          </button>
        ) : null}
        {onPin ? (
          <button type="button" data-action="memory-pin" onClick={onPin} className={primaryActionClass}>
            定下来
          </button>
        ) : null}
        {onDelete ? (
          <button type="button" data-action="memory-delete" onClick={onDelete} className={actionClass}>
            删
          </button>
        ) : null}
      </div>
    </div>
  );
}

function Editor({
  item,
  revisions,
  onCancel,
  onSave,
}: {
  item: MemoryItemView;
  revisions: MemoryRevisionView[];
  onCancel: () => void;
  onSave: (next: { title: string; summary: string; body: string }) => void;
}) {
  const [title, setTitle] = useState(item.title);
  const [summary, setSummary] = useState(item.summary);
  const [body, setBody] = useState(item.body);
  return (
    <div data-kind="memory-editor" className="mt-2 rounded-btn border border-line-2 p-3">
      <input
        data-kind="memory-editor-title"
        value={title}
        onChange={(event) => setTitle(event.target.value)}
        className="mb-2 w-full rounded-btn border border-line-2 bg-content px-2 py-1 text-3 text-ink"
      />
      <textarea
        data-kind="memory-editor-summary"
        value={summary}
        onChange={(event) => setSummary(event.target.value)}
        placeholder="摘要"
        className="mb-2 w-full rounded-btn border border-line-2 bg-content px-2 py-1 text-2 text-ink"
      />
      <textarea
        data-kind="memory-editor-body"
        value={body}
        onChange={(event) => setBody(event.target.value)}
        rows={4}
        className="mb-2 w-full rounded-btn border border-line-2 bg-content px-2 py-1 text-2 text-ink"
      />
      <div className="flex items-center gap-3">
        <button
          type="button"
          data-action="memory-save"
          onClick={() => onSave({ title, summary, body })}
          className={primaryActionClass}
        >
          保存（出新一版）
        </button>
        <button type="button" data-action="memory-cancel" onClick={onCancel} className={actionClass}>
          取消
        </button>
      </div>
      {revisions.length > 0 ? (
        <div data-kind="memory-revisions" className="mt-3">
          <p className="mb-1 text-2 text-ink-3">历次版本</p>
          {revisions.map((revision) => (
            <p key={revision.revision} className="text-1 text-ink-3">
              第 {revision.revision} 版 · {revision.reason} · {clock(revision.createdAt)} ·{' '}
              {revision.title || '（无标题）'}
            </p>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function DreamRun({ run, onUndoAll, onUndoOne }: {
  run: MemoryDreamRunView;
  onUndoAll: () => void;
  onUndoOne: (memoryId: string) => void;
}) {
  return (
    <div data-kind="dream-run" data-run-id={run.id} className="border-t border-line-2 py-2">
      <div className="flex items-center gap-2 text-2 text-ink-3">
        <span>{clock(run.startedAt)}</span>
        <span>· {run.outcome === 'done' ? '整理完了' : run.outcome === 'failed' ? '没跑成' : '在跑'}</span>
        <span>· {run.changes.length} 处</span>
        {run.outcome === 'done' && run.changes.some((change) => change.reversible) ? (
          <button type="button" data-action="dream-undo" onClick={onUndoAll} className={actionClass}>
            全部撤销
          </button>
        ) : null}
      </div>
      {run.error ? <p className="mt-1 text-2 text-fail">{run.error}</p> : null}
      {run.changes.map((change) => (
        <div
          key={change.memoryId}
          data-kind="dream-change"
          data-memory-id={change.memoryId}
          className="mt-1 flex items-start gap-2 text-2 text-ink-2"
        >
          <span className="shrink-0 text-ink-3">
            {change.action === 'new_note'
              ? '新记'
              : change.action === 'merge'
                ? '合并'
                : change.action === 'retire'
                  ? '作废'
                  : '改正'}
          </span>
          <span className="min-w-0 flex-1 whitespace-pre-wrap">
            {change.title || change.reason}
            {change.reason ? <span className="text-ink-3">（{change.reason}）</span> : null}
          </span>
          {change.reversible ? (
            <button
              type="button"
              data-action="dream-undo-change"
              onClick={() => onUndoOne(change.memoryId)}
              className={actionClass}
            >
              撤销
            </button>
          ) : (
            <span className="shrink-0 text-1 text-ink-3">你后来改过，跳过</span>
          )}
        </div>
      ))}
    </div>
  );
}

export function MemoryScene() {
  const { data, loading, error, query, setQuery, load, run } = useMemoryState();
  const [editing, setEditing] = useState<{ item: MemoryItemView; revisions: MemoryRevisionView[] } | null>(
    null,
  );
  const [intervalHours, setIntervalHours] = useState<number>(24);

  useEffect(() => mountMemoryLive(), []);

  useEffect(() => {
    if (data) setIntervalHours(data.dream.intervalHours);
  }, [data?.dream.intervalHours]);

  const startEdit = async (item: MemoryItemView) => {
    setEditing({ item, revisions: [] });
    try {
      const detail = await memoryRead({ itemId: item.id });
      setEditing((current) =>
        current && current.item.id === item.id
          ? { item: current.item, revisions: detail.itemRevisions }
          : current,
      );
    } catch {
      // 历史读不出来不影响改。
    }
  };

  const act = (action: MemoryActionKind) => {
    void run(action);
  };

  const notes = data?.notes ?? [];
  const bindingEntries = useMemo(
    () => (data?.entries ?? []).filter((item) => item.binding === 'binding'),
    [data?.entries],
  );
  const fallibleEntries = useMemo(
    () => (data?.entries ?? []).filter((item) => item.binding !== 'binding'),
    [data?.entries],
  );
  const latestRun = data?.dream.runs[0];

  return (
    <div data-kind="memory-page" className="h-full overflow-y-auto p-4">
      <div className="mx-auto flex max-w-3xl flex-col gap-4">
        <header className="flex items-center gap-2">
          <Brain className="size-5 text-brand" strokeWidth={1.7} />
          <h1 className="text-4 font-medium text-ink">记忆</h1>
          <span className="text-2 text-ink-3">关于你、Kind 的笔记和日记；都是你自己看得见的东西。</span>
        </header>

        <Section title="搜" hint="记忆、日记、Kind 对话，带出处">
          <form
            className="flex items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void load(query);
            }}
          >
            <div className="flex min-w-0 flex-1 items-center gap-2 rounded-btn border border-line-2 bg-content px-2 py-1">
              <Search className="size-3.5 text-ink-3" strokeWidth={1.8} />
              <input
                data-kind="memory-search-input"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="搜记忆、日记、Kind 对话…"
                className="min-w-0 flex-1 bg-transparent text-3 text-ink outline-none"
              />
            </div>
            <button type="submit" data-action="memory-search" className={primaryActionClass}>
              搜
            </button>
            {loading ? <span className="text-2 text-ink-3">读…</span> : null}
          </form>
          {error ? <p className="mt-2 text-2 text-fail">{error}</p> : null}
          <div data-kind="memory-search-results" className="mt-2">
            {(data?.search ?? []).map((hit, index) => (
              <div
                key={hit.kind + hit.id + String(index)}
                data-kind="memory-hit"
                data-hit-kind={hit.kind}
                className="border-b border-line-2 py-2 text-2 last:border-b-0"
              >
                <div className="flex items-center gap-2 text-ink">
                  <span className="shrink-0 text-ink-3">
                    {hit.kind === 'memory' ? '记忆' : hit.kind === 'diary' ? '日记' : 'Kind 对话'}
                  </span>
                  <span className="min-w-0 truncate">{hit.title}</span>
                  <span className="shrink-0 text-1 text-ink-3">
                    {hit.kind === 'conversation'
                      ? '出处 ' + (hit.conversationId ?? '') + ':' + String(hit.seq ?? '')
                      : '出处 ' + hit.id}
                  </span>
                </div>
                <p className="mt-0.5 line-clamp-2 text-ink-2">{hit.snippet}</p>
              </div>
            ))}
            {query.trim() && (data?.search ?? []).length === 0 ? (
              <p className="py-2 text-2 text-ink-3">没搜到。</p>
            ) : null}
          </div>
        </Section>

        <Section title="关于你" hint="只有你和采纳能写；改出新一版">
          <div data-kind="memory-about">
            {(data?.aboutYou ?? []).map((item) => (
              <div key={item.id}>
                <ItemRow
                  item={item}
                  onEdit={() => void startEdit(item)}
                  onDelete={() => act({ kind: 'delete', id: item.id })}
                />
                {editing?.item.id === item.id ? (
                  <Editor
                    item={editing.item}
                    revisions={editing.revisions}
                    onCancel={() => setEditing(null)}
                    onSave={(next) => {
                      act({
                        kind: 'correct',
                        id: item.id,
                        title: next.title,
                        summary: next.summary,
                        body: next.body,
                        reason: 'user',
                      });
                      setEditing(null);
                    }}
                  />
                ) : null}
              </div>
            ))}
            {(data?.aboutYou ?? []).length === 0 ? (
              <p className="text-2 text-ink-3">还没有。你对 Kind 说「记住……」就会有一条。</p>
            ) : null}
          </div>
        </Section>

        <Section title="条目" hint="左边范围；右边分你定的和 Kind 记的">
          <div data-kind="memory-entries" className="flex flex-col gap-4">
            <div>
              <p className="mb-1 text-2 text-ink-3">你定的</p>
              <div data-kind="memory-group" data-group="binding">
                {bindingEntries.map((item) => (
                  <div key={item.id}>
                    <ItemRow
                      item={item}
                      onEdit={() => void startEdit(item)}
                      onDelete={() => act({ kind: 'delete', id: item.id })}
                    />
                    {editing?.item.id === item.id ? (
                      <Editor
                        item={editing.item}
                        revisions={editing.revisions}
                        onCancel={() => setEditing(null)}
                        onSave={(next) => {
                          act({
                            kind: 'correct',
                            id: item.id,
                            title: next.title,
                            summary: next.summary,
                            body: next.body,
                            reason: 'user',
                          });
                          setEditing(null);
                        }}
                      />
                    ) : null}
                  </div>
                ))}
                {bindingEntries.length === 0 ? (
                  <p className="py-1 text-2 text-ink-3">还没有；在 Kind 记的笔记上点「定下来」。</p>
                ) : null}
              </div>
            </div>
            <div>
              <p className="mb-1 text-2 text-ink-3">Kind 记的</p>
              <div data-kind="memory-group" data-group="note">
                {[...notes, ...fallibleEntries].map((item) => (
                  <div key={item.id}>
                    <ItemRow
                      item={item}
                      onEdit={item.layer === 'note' ? undefined : () => void startEdit(item)}
                      onPin={item.layer === 'note' ? () => act({ kind: 'pin', id: item.id }) : undefined}
                      onDelete={() => act({ kind: 'delete', id: item.id })}
                    />
                  </div>
                ))}
                {notes.length + fallibleEntries.length === 0 ? (
                  <p className="py-1 text-2 text-ink-3">还没有；Kind 平时会自己记几笔。</p>
                ) : null}
              </div>
            </div>
          </div>
        </Section>

        <Section title="Kind 的日记" hint="只读；压缩前会记一笔">
          <div data-kind="memory-diary">
            {(data?.diary ?? []).map((entry) => (
              <div
                key={entry.id}
                data-kind="memory-diary-item"
                className="border-b border-line-2 py-2 text-2 last:border-b-0"
              >
                <p className="text-1 text-ink-3">
                  {entry.day || dayOf(entry.createdAt)} · {clock(entry.createdAt)}
                </p>
                <p className="whitespace-pre-wrap text-ink-2">{entry.text}</p>
              </div>
            ))}
            {(data?.diary ?? []).length === 0 ? (
              <p className="text-2 text-ink-3">还没有日记。</p>
            ) : null}
          </div>
        </Section>

        <Section title="做梦" hint="空闲时整理 Kind 自己的笔记；可关、可随时叫">
          <div data-kind="memory-dream" className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center gap-3 text-2 text-ink-2">
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  data-kind="dream-enabled"
                  checked={data?.dream.enabled ?? false}
                  onChange={(event) =>
                    act({ kind: 'set-dream-enabled', enabled: event.target.checked })
                  }
                />
                开着
              </label>
              <label className="flex items-center gap-2">
                间隔
                <input
                  type="number"
                  min={1}
                  data-kind="dream-interval"
                  value={intervalHours}
                  onChange={(event) => setIntervalHours(Number(event.target.value) || 1)}
                  className="w-16 rounded-btn border border-line-2 bg-content px-2 py-0.5 text-2 text-ink"
                />
                小时
              </label>
              <button
                type="button"
                data-action="dream-interval-save"
                onClick={() => act({ kind: 'set-dream-interval', hours: intervalHours })}
                className={actionClass}
              >
                保存间隔
              </button>
              {data?.dream.running ? (
                <span className="text-ink-3" data-kind="dream-running">
                  正在整理
                </span>
              ) : (
                <button
                  type="button"
                  data-action="dream-now"
                  onClick={() => act({ kind: 'dream-now' })}
                  className={primaryActionClass}
                >
                  马上整理一次
                </button>
              )}
              {data && !data.dream.permit ? (
                <button
                  type="button"
                  data-action="dream-permit"
                  onClick={() => act({ kind: 'set-permit', allow: true })}
                  className={primaryActionClass}
                >
                  允许观察（读对话和任务报回）
                </button>
              ) : null}
            </div>
            <p className="text-2 text-ink-3">
              上次成功 {clock(data?.dream.lastSuccessAt)} · 下次 {clock(data?.dream.nextAt)}
            </p>
            <div data-kind="dream-runs">
              {latestRun ? (
                <DreamRun
                  run={latestRun}
                  onUndoAll={() => act({ kind: 'undo-dream', runId: latestRun.id })}
                  onUndoOne={(memoryId) =>
                    act({ kind: 'undo-dream', runId: latestRun.id, memoryId })
                  }
                />
              ) : (
                <p className="text-2 text-ink-3">还没整理过。</p>
              )}
            </div>
          </div>
        </Section>

        <Section title="隔离区" hint="删掉的 7 天内能恢复；彻底删就搜不到了">
          <div data-kind="memory-quarantine">
            {(data?.quarantine ?? []).map((item) => (
              <div
                key={item.id}
                data-kind="memory-quarantine-item"
                data-id={item.id}
                className="flex items-center gap-2 border-b border-line-2 py-2 last:border-b-0"
              >
                <Bookmark className="size-3 shrink-0 text-wait" strokeWidth={1.7} />
                <span className="min-w-0 flex-1 truncate text-2 text-ink">{item.title}</span>
                <button
                  type="button"
                  data-action="memory-restore"
                  onClick={() => act({ kind: 'restore', id: item.id })}
                  className={actionClass}
                >
                  <RotateCcw className="mr-1 inline size-3" strokeWidth={1.8} />
                  恢复
                </button>
                <button
                  type="button"
                  data-action="memory-forget"
                  onClick={() => act({ kind: 'forget', id: item.id })}
                  className={actionClass}
                >
                  <Trash2 className="mr-1 inline size-3" strokeWidth={1.8} />
                  彻底删除
                </button>
              </div>
            ))}
            {(data?.quarantine ?? []).length === 0 ? (
              <p className="text-2 text-ink-3">空。</p>
            ) : null}
          </div>
        </Section>
      </div>
    </div>
  );
}
