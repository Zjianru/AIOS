/**
 * 设置场景（#settings、#settings:<分组>）。
 *
 * 分组照 v13：外观、模型与连接、Kind、执行、通知、隐私与更新、监控、关于。
 * 现在只做了「模型与连接」（它后面就是 Keyring，用户 2026-11-01 定：设置页现在就做，D47）；
 * 其余分组仍是第 3 期，先放一行说明。默认落到「外观」，照 ROUTES.md。
 */

import { go } from '@/app/navigate';
import { SETTINGS_GROUPS } from '@/app/route';
import { useRouteStore } from '@/app/routeStore';
import { cn } from '@/lib/utils';
import { ModelSettings } from './ModelSettings';

/** 已经做出来的分组；其余仍是第 3 期。 */
const BUILT = new Set(['model']);
const GROUPS = SETTINGS_GROUPS.map((group) => ({ ...group, built: BUILT.has(group.id) }));

export function SettingsScene() {
  const route = useRouteStore((state) => state.route);
  // 只显示接上内核的部分（D46）：设置里只有「模型与连接」。
  const groups = GROUPS.filter((group) => group.built);
  const current = groups.find((group) => group.id === route.id) ?? groups[0];

  return (
    <div className="flex h-full min-h-0" data-scene-page="settings">
      <nav className="w-[184px] shrink-0 border-r border-line px-2 py-4" aria-label="设置分组">
        <div className="px-2 pb-2 text-1 text-ink-3">设置</div>
        {groups.map((group) => (
          <button
            key={group.id}
            type="button"
            onClick={() => go('settings:' + group.id)}
            aria-current={group.id === current.id ? 'page' : undefined}
            className={cn(
              'flex w-full items-center rounded-btn px-2 py-1.5 text-left text-3 transition-colors',
              group.id === current.id ? 'bg-hover text-ink' : 'text-ink-2 hover:bg-hover hover:text-ink',
            )}
          >
            {group.name}
            {group.built ? null : <span className="ml-auto text-1 text-ink-3">第 3 期</span>}
          </button>
        ))}
      </nav>

      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[720px] px-8 py-8">
          <h1 className="text-5 font-medium text-ink">{current.name}</h1>
          {current.id === 'model' ? (
            <ModelSettings />
          ) : (
            <p className="mt-3 text-3 leading-6 text-ink-2">
              这一组排在第 3 期，样子见 v13 原型的「设置 › {current.name}」。
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
