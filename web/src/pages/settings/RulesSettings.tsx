import { useCallback, useEffect, useState } from 'react';
import type { Rule } from '@/data/generated/Rule';
import type { RulesReadResponse } from '@/data/generated/RulesReadResponse';
import { ruleDelete, ruleSave, rulesRead } from '@/data/service';
import { toast } from 'sonner';

const EMPTY: Rule = {
  id: '',
  rev: 0,
  scope: 'global',
  category: '动作',
  target: '',
  strictness: 'ask',
  text: '',
  source: '用户',
};

export function RulesSettings() {
  const [view, setView] = useState<RulesReadResponse | null>(null);
  const [draft, setDraft] = useState<Rule>(EMPTY);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await rulesRead());
    } catch (error) {
      toast.error((error as Error).message);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    if (!draft.id.trim() || !draft.text.trim()) {
      toast.error('规则 id 和内容都要填');
      return;
    }
    setBusy(true);
    try {
      setView(await ruleSave(draft));
      setDraft(EMPTY);
      toast.success('规则保存了；下一轮生效');
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    setBusy(true);
    try {
      setView(await ruleDelete(id));
      toast.success('规则删了');
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mt-6" data-settings="rules">
      <h2 className="text-3 font-medium text-ink">规则</h2>
      <p className="mt-1 text-2 leading-5 text-ink-2">
        全局和项目两层；越近的范围只能更严或相同。规则只进上下文，不做程序侧拦截。
      </p>

      {view && view.conflicts.length > 0 ? (
        <div data-rules-conflicts className="mt-3 rounded-card border border-wait/40 bg-wait-soft p-3 text-2 text-wait">
          {view.conflicts.map((conflict) => (
            <div key={conflict.ruleId + conflict.overriddenBy}>
              {conflict.ruleId} 被 {conflict.overriddenBy} 压住：{conflict.reason}
            </div>
          ))}
        </div>
      ) : null}

      <div className="mt-3 space-y-2" data-rules-list>
        {(view?.rules ?? []).map((rule) => (
          <div key={rule.id} data-rule-id={rule.id} className="rounded-card border border-line p-3">
            <div className="flex flex-wrap items-center gap-2 text-2">
              <span className="font-medium text-ink">{rule.id}</span>
              <span className="text-ink-3">{rule.scope}</span>
              <span className="text-ink-3">{rule.category}</span>
              <span className="text-ink-3">{rule.strictness}</span>
              <span className="ml-auto text-1 text-ink-3">v{rule.rev} · {rule.source}</span>
              <button type="button" data-rule-delete={rule.id} disabled={busy} onClick={() => void remove(rule.id)} className="text-1 text-fail hover:underline">删除</button>
            </div>
            <p className="mt-1 text-2 leading-5 text-ink-2">{rule.text}</p>
          </div>
        ))}
        {view && view.rules.length === 0 ? <p className="text-2 text-ink-3">还没有规则。</p> : null}
      </div>

      <div className="mt-4 rounded-card border border-line p-3" data-rules-editor>
        <div className="text-2 text-ink">新建 / 修改</div>
        <div className="mt-2 flex flex-wrap gap-2">
          <input data-rule-id-input value={draft.id} onChange={(event) => setDraft({ ...draft, id: event.target.value })} placeholder="规则 id" className="w-40 rounded-btn border border-line bg-pane px-2 py-1 text-2 text-ink outline-none" />
          <select data-rule-scope value={draft.scope} onChange={(event) => setDraft({ ...draft, scope: event.target.value })} className="rounded-btn border border-line bg-pane px-2 py-1 text-2 text-ink">
            <option value="global">全局</option>
            <option value="project:p1">项目 p1</option>
          </select>
          <select data-rule-strictness value={draft.strictness} onChange={(event) => setDraft({ ...draft, strictness: event.target.value })} className="rounded-btn border border-line bg-pane px-2 py-1 text-2 text-ink">
            <option value="deny">禁用</option>
            <option value="ask">需确认</option>
            <option value="allow">允许</option>
          </select>
          <input data-rule-category-input value={draft.category} onChange={(event) => setDraft({ ...draft, category: event.target.value })} placeholder="类别" className="w-24 rounded-btn border border-line bg-pane px-2 py-1 text-2 text-ink outline-none" />
        </div>
        <textarea data-rule-text value={draft.text} onChange={(event) => setDraft({ ...draft, text: event.target.value })} placeholder="规则内容" rows={2} className="mt-2 w-full rounded-btn border border-line bg-pane px-2 py-1 text-2 text-ink outline-none" />
        <button type="button" data-rule-save disabled={busy} onClick={() => void save()} className="mt-2 rounded-btn bg-active px-3 py-1.5 text-2 text-ink">保存规则</button>
      </div>

      <p className="mt-2 text-1 text-ink-3">当前版本 v{view?.version ?? 0}；规则改动下一轮生效。</p>
    </section>
  );
}
