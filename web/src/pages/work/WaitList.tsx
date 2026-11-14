/**
 * T055：「等你」列表（工作页右栏 /「现在」的等你决定共用）。
 *
 * 五种按固定顺序分组；每项有词、原阶段、标题、要你做什么（一句）、摘要/问题原文（两行折起）、
 * 命中多种时的「还有：…」。「去看/去判/去答」回 Kind 对话里出那张卡的那一轮。
 */

import { toast } from 'sonner';
import { go } from '@/app/navigate';
import type { WaitItem } from '@/data/generated/WaitItem';
import { useKindStore } from '@/pages/kind/kindStore';
import { cn } from '@/lib/utils';
import { WAIT_ACTION, WAIT_LABEL, stageLabel, waitGroups } from './workModel';

/** 让 Kind 页挂载并把第 turn 轮滚出来；返回是否真的滚到了。 */
async function revealKindTurn(turn: number): Promise<boolean> {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    if (await useKindStore.getState().revealTurnNumber(turn)) return true;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return false;
}

/** 点「去看/去判/去答」：先回 Kind 滚到那一轮；找不到再退回任务对话或提示。 */
export async function openWaitItem(item: WaitItem): Promise<void> {
  if (item.turn !== undefined && item.turn !== null) {
    const before = useKindStore.getState().loadCount;
    go('kind');
    // 等 Kind 页新一屏加载完再滚，别拿上一次的旧视图滚。
    for (let attempt = 0; attempt < 60; attempt += 1) {
      if (useKindStore.getState().loadCount > before) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    // T055 审计 F3：用 wait 项的轮号选同一任务的最新结果卡（不能按 taskId 选最早旧卡）；
    // 任务本身没有结果卡（例如绑总任务的确认卡）再按轮号滚。
    if (
      item.taskId &&
      (await useKindStore.getState().revealTaskResult(item.taskId, item.turn))
    ) {
      return;
    }
    if (await revealKindTurn(item.turn)) return;
  }
  if (item.taskId) {
    go('taskchat:' + item.taskId);
    return;
  }
  toast('那一轮更早，不在当前历史里');
}

/** 一行「词 · 原阶段 · 标题 · 要你做什么 · 摘要/问题（两行）· 还有：…」。 */
export function WaitRow({ item, className }: { item: WaitItem; className?: string }) {
  const action = WAIT_ACTION[item.kind];
  return (
    <article
      data-kind="wait-item"
      data-wait-kind={item.kind}
      className={cn(
        'rounded-info border border-line bg-pane/50 px-2.5 py-2 transition-colors hover:bg-raised',
        className,
      )}
    >
      <div className="flex flex-wrap items-baseline gap-x-1.5 gap-y-0.5 text-1 text-ink-3">
        <span data-kind="wait-word" className="font-medium text-wait">
          {WAIT_LABEL[item.kind] ?? item.kind}
        </span>
        {item.stage ? (
          <span data-kind="wait-stage" className="rounded-badge bg-hover px-1.5 py-[1px]">
            {stageLabel(item.stage)}
          </span>
        ) : null}
        <span className="ml-auto brand-type">{item.turn !== undefined && item.turn !== null ? '第 ' + item.turn + ' 轮' : ''}</span>
      </div>
      <div className="mt-0.5 truncate text-2 text-ink" title={item.title}>
        {item.title}
      </div>
      <div className="mt-0.5 line-clamp-2 text-1 leading-4 text-ink-2" data-kind="wait-body">
        {item.body}
      </div>
      {item.also?.length ? (
        <div className="mt-0.5 text-1 text-ink-3" data-kind="wait-also">
          还有：{item.also.map((kind) => WAIT_LABEL[kind] ?? kind).join('、')}
        </div>
      ) : null}
      <div className="mt-1 flex items-center gap-2">
        <span className="text-1 text-ink-3" data-kind="wait-need">
          {item.need}
        </span>
        {action ? (
          <button
            type="button"
            data-action="wait-open"
            onClick={() => void openWaitItem(item)}
            className="ml-auto flex items-center gap-0.5 rounded-btn px-1.5 py-0.5 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2"
          >
            {action} <span aria-hidden="true">›</span>
          </button>
        ) : null}
      </div>
    </article>
  );
}

/** 按五种分组；右栏和「现在」的「等你决定」共用。 */
export function WaitList({ items, emptyText }: { items: WaitItem[]; emptyText?: string }) {
  const groups = waitGroups(items);
  if (groups.length === 0) {
    return <p className="px-1 py-2 text-1 text-ink-3">{emptyText ?? '没有要你的事。'}</p>;
  }
  return (
    <div className="space-y-3" data-kind="wait-list">
      {groups.map((group) => (
        <section key={group.kind} data-kind="wait-group" data-wait-group={group.kind}>
          <header className="mb-1 flex items-baseline gap-1.5 px-0.5 text-1">
            <span className="text-wait">{group.label}</span>
            <span className="brand-type text-ink-3">{group.items.length}</span>
          </header>
          <div className="space-y-1.5">
            {group.items.map((item, index) => (
              <WaitRow key={item.kind + ':' + (item.taskId ?? item.title) + ':' + String(index)} item={item} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
