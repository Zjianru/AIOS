/**
 * 底栏的模型选择（docs 10 篇 §10.2：这里只选，不加不改；加连接、改连接去设置）。
 *
 * - 列出已保存的连接和它们的模型；连接不可用的灰掉，说原因。
 * - 选了从下一轮开始用；正在跑的这一轮不换。
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

function usable(view: ModelSettingsView, connectionId: string): { ok: true } | { ok: false; why: string } {
  if (view.keyFile.problem) return { ok: false, why: '读不到 Keyring' };
  const conn = view.connections.find((item) => item.id === connectionId);
  if (!conn || !conn.key.saved) return { ok: false, why: '还没有密钥' };
  if (conn.check.kind === 'fail') return { ok: false, why: '连接不可用' };
  return { ok: true };
}

export function ModelPicker() {
  const model = useKindStore((state) => state.model);
  const setModel = useKindStore((state) => state.setModel);
  const [view, setView] = useState<ModelSettingsView | null>(null);

  return (
    <DropdownMenu onOpenChange={(open) => open && void getModelSettings().then(setView)}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex items-center gap-1 rounded-btn px-1 py-[1px] text-ink-2 transition-colors hover:bg-hover hover:text-ink"
          data-kind="model-picker"
        >
          {model.label}
          <ChevronDown className="size-3 opacity-70" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="w-64">
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
                        // T008 r4 #3：等 selectSessionModel 成功后再改状态/tost；失败给中文原因，不吞。
                        void selectSessionModel({ model: row.id })
                          .then(() =>
                            setModel({ connectionId: conn.id, modelId: row.id, label: row.name ?? row.id }),
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
                        <span className="brand-type text-1 text-ink-3">{Math.round(row.contextWindow / 1000)}k</span>
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
