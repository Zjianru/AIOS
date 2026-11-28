/**
 * 通用页面 Kind 栏（D118/D131）：设置页右侧就是同一条 Kind 对话。
 *
 * - 页面只给「在看：设置 · 模型与连接」和几条常说的句子；点一句就是把这句话发进 Kind 对话，
 *   不本地写回复、不本地造卡。
 * - 消息、回答、确认卡都来自 useKindStore 的同一条根 Kind 会话；卡确认后设置页通过
 *   aios:settings-changed 重读，页面按钮和 Kind 走同一条 settings_update。
 * - 密钥类动作 Kind 不经手；诊断包要用户自己确认；这些约束由 Kind 的 skill/规则保证，
 *   这里不另做一份。
 */

import { useEffect, useMemo, useRef, useState } from 'react';
import { Sparkles, SendHorizontal, X } from 'lucide-react';
import { go } from '@/app/navigate';
import { cn } from '@/lib/utils';
import { useKindStore } from '@/pages/kind/kindStore';
import { TurnRow } from '@/pages/kind/TurnRow';

export interface PageKindPage {
  /** 路由目标，例如 settings:model。 */
  path: string;
  /** 头部「在看：…」。 */
  where: string;
}

const SUGGESTIONS: Record<string, string[]> = {
  model: ['加一个看图模型', '打开截桌面', '关掉截桌面', '看图模型应该怎么填？'],
  exec: ['席位加到 5 个', '先全局暂停', '打开 bash', '关掉 bash'],
  plugins: ['打开 Office 插件', '关掉 Office 插件'],
  rules: ['加一条全局规则', '现在有哪些规则？'],
  notify: ['打开系统通知', '关掉系统通知', '再查一次通知权限'],
  look: ['把主题改成跟随系统', '打开减少动效', '关掉减少动效'],
  about: ['导出诊断包', '数据目录在哪？'],
};

export function PageKind({ page }: { page: PageKindPage }) {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const bootstrap = useKindStore((state) => state.bootstrap);
  const send = useKindStore((state) => state.send);
  const views = useKindStore((state) => state.views);
  const gate = useKindStore((state) => state.gate);
  const confirmedRef = useRef('');
  const group = page.path.includes(':') ? page.path.split(':')[1] : 'model';

  useEffect(() => {
    if (open) void bootstrap();
  }, [open, bootstrap]);

  // 卡确认后由这里通知设置页重读；不轮询。
  const confirmed = useMemo(
    () => views.filter((view) => Boolean(view.card?.decision)).map((view) => view.id).join(','),
    [views],
  );
  useEffect(() => {
    if (!confirmed || confirmed === confirmedRef.current) return;
    confirmedRef.current = confirmed;
    window.dispatchEvent(new CustomEvent('aios:settings-changed'));
  }, [confirmed]);

  const sendText = async (raw: string) => {
    const value = raw.trim();
    if (!value || sending) return;
    setSending(true);
    try {
      // 页面来源是这条用户消息的 source；后端把它作为程序加的一行放在消息块末尾。
      const accepted = await send(value, {
        source: { path: page.path, title: page.where, group },
      });
      if (accepted) setText('');
    } finally {
      setSending(false);
    }
  };

  const submit = async () => {
    await sendText(text);
  };

  const shown = views.slice(-3);
  const suggestions = SUGGESTIONS[group] ?? [];

  return (
    <>
      <button
        type="button"
        data-kind="page-kind-open"
        onClick={() => setOpen(true)}
        className="fixed bottom-5 right-5 z-30 inline-flex items-center gap-1.5 rounded-btn border border-line bg-raised px-2.5 py-1.5 text-2 text-ink-2 shadow-[var(--shadow-pop)] transition-colors hover:bg-hover hover:text-ink"
        title="让 Kind 替你做这一页上的事：它出卡，你点了才改"
      >
        <Sparkles className="size-3.5" />
        页 Kind
      </button>
      {open ? (
        <aside
          data-kind="page-kind"
          data-page-kind={group}
          className="fixed bottom-16 right-5 z-30 flex max-h-[min(680px,78vh)] w-[380px] flex-col rounded-card border border-line-2 bg-raised shadow-[var(--shadow-pop)]"
        >
          <header className="flex items-center gap-2 border-b border-line px-3.5 py-2.5">
            <Sparkles className="size-4 text-run" />
            <div className="min-w-0 flex-1">
              <div className="text-3 font-medium leading-5 text-ink">Kind</div>
              <div className="truncate text-1 leading-4 text-ink-3">在看：{page.where}</div>
            </div>
            <button
              type="button"
              onClick={() => go('kind')}
              className="rounded-btn px-1.5 py-1 text-1 text-ink-3 hover:bg-hover hover:text-ink"
              title="到 Kind 对话里看"
            >
              到对话里看
            </button>
            <button type="button" onClick={() => setOpen(false)} className="grid size-6 place-items-center rounded-btn text-ink-3 hover:bg-hover hover:text-ink" aria-label="关上">
              <X className="size-3.5" />
            </button>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-3" data-kind="page-kind-conversation">
            {gate !== 'ok' ? (
              <p className="text-2 text-ink-3">Kind 对话现在不可用：内核还没接上。到 Kind 页看连接说明。</p>
            ) : (
              <>
                {shown.length === 0 ? (
                  <p className="text-2 leading-6 text-ink-2">这一页上的事可以直接跟我说。要改东西我出一张卡，你点了才改。</p>
                ) : (
                  shown.map((view) => (
                    <TurnRow
                      key={view.id}
                      view={view}
                      dayLabel={null}
                      showLastSeen={false}
                      lastSeenCounts={{ moved: 0, waiting: 0 }}
                      onCanvasMessage={() => {}}
                    />
                  ))
                )}
                {suggestions.length > 0 ? (
                  <div className="mt-3 border-t border-line pt-2">
                    <div className="text-1 text-ink-3">常说的</div>
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {suggestions.map((say) => (
                        <button
                          key={say}
                          type="button"
                          data-kind="page-kind-suggestion"
                          onClick={() => {
                            void sendText(say);
                          }}
                          className={cn('rounded-btn border border-line px-2 py-0.5 text-left text-1 leading-5 text-ink-2 transition-colors hover:border-line-2 hover:bg-hover hover:text-ink')}
                        >
                          {say}
                        </button>
                      ))}
                    </div>
                  </div>
                ) : null}
              </>
            )}
          </div>
          <form
            className="flex items-end gap-2 border-t border-line p-2.5"
            onSubmit={(event) => {
              event.preventDefault();
              void submit();
            }}
          >
            <textarea
              data-kind="page-kind-input"
              value={text}
              rows={2}
              onChange={(event) => setText(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  void submit();
                }
              }}
              placeholder={'跟 Kind 说' + page.where + '的事…'}
              className="min-h-0 flex-1 resize-none bg-transparent text-2 leading-5 text-ink outline-none placeholder:text-ink-3"
            />
            <button
              type="submit"
              data-kind="page-kind-send"
              disabled={!text.trim() || sending}
              className="grid size-7 shrink-0 place-items-center rounded-full bg-ink text-content disabled:opacity-30"
              aria-label="发送"
            >
              <SendHorizontal className="size-3.5" />
            </button>
          </form>
        </aside>
      ) : null}
    </>
  );
}
