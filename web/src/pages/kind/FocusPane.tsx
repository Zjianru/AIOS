/**
 * Kind 右栏「聚焦」（brief §四.1「第 2 期追加」第 1 条）。
 *
 * 上半块：只放要你看的和正在动的——等你 → 出了问题 → 在跑，每组最多 3 条；
 * 其余正常推进折成一行；一条都没有就只显示「都在正常推进」，高度随内容收缩。
 * 数据只从 api 取；归约在 focusModel.ts，组件里不做业务计算。
 */

import { useEffect, useState } from 'react';
import { listInProgressTasks, listProjects, listTasks } from '@/data/api';
import { go } from '@/app/navigate';
import { MiniLineStrip } from '@/components/subway/MiniLineStrip';
import { buildFocusView } from './focusModel';
import type { FocusItem, FocusState, FocusView } from './focusModel';

const STATE_COLOR: Record<FocusState, string> = {
  wait: 'var(--wait)',
  fail: 'var(--fail)',
  run: 'var(--run)',
};

const GROUP_LABEL: Record<FocusState, string> = {
  wait: '等你',
  fail: '出了问题',
  run: '在跑',
};

function FocusRow({ item }: { item: FocusItem }) {
  return (
    <button
      type="button"
      onClick={() => go(item.href)}
      className="mt-0.5 block w-full rounded-info px-2 py-1.5 text-left transition-colors hover:bg-hover"
    >
      <span className="flex items-baseline justify-between gap-2">
        <span className="truncate text-2 text-ink">{item.title}</span>
        <span className="shrink-0 text-1 text-ink-3">{item.right}</span>
      </span>
      <span className="mt-0.5 flex items-center gap-1.5 text-1 text-ink-3">
        <span
          className="grid size-[14px] shrink-0 place-items-center rounded-badge brand-type text-[9px] font-semibold text-white"
          style={{ backgroundColor: item.project.color }}
        >
          {item.project.code}
        </span>
        <span className="truncate">{item.meta}</span>
      </span>
      {item.strip ? <MiniLineStrip strip={item.strip} /> : null}
    </button>
  );
}

export function FocusPane() {
  const [view, setView] = useState<FocusView | null>(null);

  useEffect(() => {
    let alive = true;
    void (async () => {
      const [roots, tasks, projects] = await Promise.all([listInProgressTasks(), listTasks(), listProjects()]);
      if (!alive) return;
      setView(buildFocusView(roots, tasks, projects));
    })();
    return () => {
      alive = false;
    };
  }, []);

  const summary = (() => {
    if (!view || view.allNormal) return '';
    const parts: string[] = [];
    if (view.toSee > 0) parts.push('要你看的 ' + view.toSee + ' 件');
    if (view.running > 0) parts.push('在跑 ' + view.running + ' 件');
    if (view.rest > 0) parts.push('其余 ' + view.rest + ' 站正常');
    return parts.join(' · ');
  })();

  return (
    <section className="shrink-0 border-b border-line" data-block="focus">
      <header className="px-4 pt-3">
        <div className="flex items-baseline gap-2">
          <h3 className="text-3 font-medium text-ink">聚焦</h3>
        </div>
        {summary ? <p className="mt-0.5 text-1 text-ink-3">{summary}</p> : null}
      </header>

      {!view ? (
        <div className="px-4 py-2 text-2 text-ink-3">正在核对…</div>
      ) : view.allNormal ? (
        <div className="px-4 pb-3 pt-2 text-2 text-ink-3">都在正常推进</div>
      ) : (
        <div className="max-h-[38vh] overflow-y-auto px-4 pb-3 pt-2">
          {view.groups.map((group) => (
            <div key={group.state} className="mt-2 first:mt-0">
              <div className="flex items-center gap-1.5 text-1 text-ink-3">
                <span className="size-[7px] rounded-full" style={{ backgroundColor: STATE_COLOR[group.state] }} />
                {GROUP_LABEL[group.state]}
              </div>
              <div className="mt-0.5">
                {group.items.map((item) => (
                  <FocusRow key={item.project.id + ':' + item.id} item={item} />
                ))}
                {group.more > 0 ? <div className="px-2 pt-1 text-1 text-ink-3">还有 {group.more} 件</div> : null}
              </div>
            </div>
          ))}

          {view.rest > 0 ? (
            <button
              type="button"
              onClick={() => go('work')}
              className="mt-2 flex w-full items-center justify-between border-t border-line pt-2 text-1 text-ink-3 transition-colors hover:text-ink-2"
            >
              <span>其余 {view.rest} 站正常推进</span>
              <span>全部 ›</span>
            </button>
          ) : null}
        </div>
      )}
    </section>
  );
}
