/**
 * 模型和思考强度（docs 10 篇 §10.2：这里只选，不加不改；加连接、改连接去设置）。
 *
 * 照原型 app-v14 1f0c920：
 * - 放在输入框里、发送键旁边：按钮上是「模型 · 思考强度」；
 * - 上面一段选思考强度：列的是当前模型自己的档、厂商自己的叫法，从弱到强；
 * - 下面列已保存的连接和它们的模型；连接不可用的灰掉，说原因；
 * - 选了从下一轮开始用；正在跑的这一轮不换。模型只在用户自己选的时候换，出错也不自动换；
 * - 最下面一行去「设置 › 模型与连接」。
 */

import { useState } from 'react';
import { toast } from 'sonner';
import { Check, ChevronDown } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { getModelSettings } from '@/data/keyring';
import { selectSessionModel, ServiceHttpError } from '@/data/service';
import type { ModelSettingsView } from '@/data/fixtures/settings';
import { go } from '@/app/navigate';
import { useKindStore } from './kindStore';
import { effortName } from './effortName';

function usable(view: ModelSettingsView, connectionId: string): { ok: true } | { ok: false; why: string } {
  if (view.keyFile.problem) return { ok: false, why: '读不到 Keyring' };
  const conn = view.connections.find((item) => item.id === connectionId);
  if (!conn || !conn.key.saved) return { ok: false, why: '还没有密钥' };
  if (conn.check.kind === 'fail') return { ok: false, why: '连接不可用' };
  return { ok: true };
}

export { effortName } from './effortName';

/** 强度图标（照 Magpie effortIcon）：四格由矮到高，亮到这一档在这个模型里排第几。 */
export function EffortIcon({ effort, levels, className }: { effort?: string; levels: string[]; className?: string }) {
  const index = effort ? levels.indexOf(effort) : -1;
  const lit = levels.length > 1 && index >= 0 ? Math.round(((index + 1) / levels.length) * 4) : 0;
  return (
    <svg viewBox="0 0 16 16" className={className ?? 'size-3.5'} aria-hidden>
      {[4, 7, 10, 13].map((height, bar) => (
        <rect
          key={height}
          x={1.25 + bar * 3.6}
          y={14.5 - height}
          width={2.6}
          height={height}
          rx={1}
          fill="currentColor"
          opacity={bar < lit ? 1 : 0.28}
        />
      ))}
    </svg>
  );
}

export function ModelPicker() {
  const model = useKindStore((state) => state.model);
  const setEffort = useKindStore((state) => state.setEffort);
  const hydrateModel = useKindStore((state) => state.hydrateModel);
  const applySelection = useKindStore((state) => state.applySelection);
  const [view, setView] = useState<ModelSettingsView | null>(null);
  // 用 store 的最新值（闭包里的 wantedEffort 可能还是上一次渲染的旧值）。
  const getWantedEffort = () => {
    const state = useKindStore.getState();
    return state.wantedEffort ?? state.model.effortActual ?? state.model.effort;
  };

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (!open) return;
        void getModelSettings().then(setView);
        // T012.9：模型目录字段（档位/价格/窗口）还没到就补一次；不阻塞页面加载。
        if (!model.efforts?.length) {
          void hydrateModel({
            connectionId: model.connectionId,
            modelId: model.modelId,
            label: model.label,
            effort: model.effort,
          });
        }
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex h-7 max-w-[240px] items-center gap-1.5 rounded-btn px-2 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink data-[state=open]:bg-hover"
          data-kind="model-picker"
        >
          <span className="truncate">{model.label}</span>
          {model.effortActual ?? model.effort ? (
            <span className="flex shrink-0 items-center gap-1 text-ink-3" data-kind="effort-now">
              <span className="opacity-60">·</span>
              <EffortIcon effort={model.effortActual ?? model.effort} levels={model.efforts ?? []} />
              {effortName(model.effortActual ?? model.effort ?? '')}
            </span>
          ) : null}
          <ChevronDown className="size-3 shrink-0 opacity-60" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" side="top" className="w-72">
        <DropdownMenuLabel className="text-1 font-normal text-ink-3">思考强度</DropdownMenuLabel>
        {model.efforts?.length ? (
          <div className="flex gap-1 px-2 pb-1.5" data-kind="effort">
            {model.efforts.map((effort) => {
              const on = (model.effortActual ?? model.effort) === effort;
              return (
                <button
                  key={effort}
                  type="button"
                  onClick={() => !on && void setEffort(effort)}
                  className={
                    'h-7 flex-1 rounded-btn text-2 transition-colors ' +
                    (on ? 'bg-ink text-content' : 'text-ink-2 hover:bg-hover hover:text-ink')
                  }
                  title={effortName(effort)}
                >
                  {effort}
                </button>
              );
            })}
          </div>
        ) : (
          <div className="px-2 pb-1.5 text-2 text-ink-3" data-kind="effort">
            {model.thinks === false ? '这个模型不思考' : '这个模型不能调思考强度'}
          </div>
        )}
        <p className="px-2 pb-1 text-1 leading-4 text-ink-3">
          从下一轮开始用；正在跑的这一轮不换。
        </p>
        <DropdownMenuSeparator />
        {!view ? (
          <div className="px-2 py-1.5 text-2 text-ink-3">正在读取…</div>
        ) : view.connections.length === 0 ? (
          <div className="px-2 py-1.5 text-2 text-ink-3">还没有接任何模型</div>
        ) : (
          view.connections.map((conn) => {
            const state = usable(view, conn.id);
            const rows = conn.models.length
              ? conn.models
              : view.catalog.find((item) => item.id === conn.id)?.defaultModels ?? [];
            return (
              <div key={conn.id}>
                <DropdownMenuLabel className="flex items-center gap-2 text-1 font-normal text-ink-3">
                  {conn.name}
                  {state.ok ? null : <span className="ml-auto text-fail">{state.why}</span>}
                </DropdownMenuLabel>
                {rows.map((row) => {
                  const on = model.connectionId === conn.id && model.modelId === row.id;
                  return (
                    <DropdownMenuItem
                      key={row.id}
                      disabled={!state.ok}
                      onSelect={() => {
                        if (on) return;
                        void selectSessionModel({
                          connection: conn.id,
                          model: row.id,
                          effortWanted: getWantedEffort(),
                        })
                          .then((selected) =>
                            applySelection({
                              connectionId: selected.connection,
                              modelId: selected.model,
                              label: row.name ?? selected.model,
                              effortWanted: selected.effortWanted,
                              effortActual: selected.effortActual,
                            }),
                          )
                          .catch((error: unknown) => {
                            const detail =
                              error instanceof ServiceHttpError ? error.message : '连不上服务，稍后再试';
                            toast.error('换模型没成：' + detail);
                          });
                      }}
                      className="gap-2 text-2"
                    >
                      <span className="min-w-0 flex-1 truncate">{row.name ?? row.id}</span>
                      {row.contextWindow ? (
                        <span className="brand-type text-1 text-ink-3">
                          {Math.round(row.contextWindow / 1000)}k
                        </span>
                      ) : null}
                      {on ? <Check className="size-3.5" /> : <span className="size-3.5" />}
                    </DropdownMenuItem>
                  );
                })}
              </div>
            );
          })
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => go('settings:model')} className="text-2 text-ink-2">
          添加或修改连接 ›
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
