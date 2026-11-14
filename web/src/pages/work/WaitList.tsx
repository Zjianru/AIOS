/** T055：「等你」列表（工作页右栏 /「现在」的等你决定共用）。
 * T055.2：结果接受可以勾选成组接受；勾时存 report_id+version，变了逐项回原因；接受后立刻重取。
 */
import { toast } from 'sonner';
import { go } from '@/app/navigate';
import type { TaskAcceptManyResponse } from '@/data/generated/TaskAcceptManyResponse';
import type { WaitItem } from '@/data/generated/WaitItem';
import { useKindStore } from '@/pages/kind/kindStore';
import { cn } from '@/lib/utils';
import { WAIT_ACTION, WAIT_LABEL, stageLabel, waitGroups } from './workModel';
import { useWorkStore } from './workStore';

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

function AcceptReceipt({ receipt }: { receipt: TaskAcceptManyResponse }) {
  const accepted = receipt.outcomes.filter((outcome) => !outcome.refused);
  const refused = receipt.outcomes.filter((outcome) => outcome.refused);
  return (
    <div
      data-kind="accept-receipt"
      className="rounded-info border border-done/30 bg-done-soft/30 px-2.5 py-2 text-1 leading-4"
    >
      <p className="text-ink-2" data-kind="accept-receipt-summary">
        接受了 {accepted.length} 项{accepted.length ? '，已关闭' : ''}
        {refused.length ? '；没记 ' + refused.length + ' 项' : ''}
      </p>
      {refused.length ? (
        <ul className="mt-1 space-y-0.5 text-ink-3" data-kind="accept-receipt-refused">
          {refused.map((outcome) => (
            <li key={outcome.taskId} className="break-words">
              <span className="text-ink-2">{outcome.title}</span>：{outcome.refused}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

export function WaitRow({ item, className }: { item: WaitItem; className?: string }) {
  const action = WAIT_ACTION[item.kind];
  const pick = item.pick;
  const selected = useWorkStore((state) => (pick ? Boolean(state.selection[pick.taskId]) : false));
  const accepting = useWorkStore((state) => state.accepting);
  const togglePick = useWorkStore((state) => state.togglePick);
  const acceptOne = useWorkStore((state) => state.acceptOne);
  return (
    <article
      data-kind="wait-item"
      data-wait-kind={item.kind}
      data-task-id={item.taskId ?? undefined}
      className={cn(
        'rounded-info border border-line bg-pane/50 px-2.5 py-2 transition-colors hover:bg-raised',
        pick && selected && 'border-done/40 bg-done-soft/20',
        className,
      )}
    >
      <div className="flex flex-wrap items-baseline gap-x-1.5 gap-y-0.5 text-1 text-ink-3">
        {pick ? (
          <input
            type="checkbox"
            data-kind="wait-pick"
            aria-label={'勾选接受：' + item.title}
            checked={selected}
            disabled={accepting}
            onChange={() => togglePick(pick)}
            className="mr-0.5 size-3 accent-[var(--done)]"
          />
        ) : null}
        <span data-kind="wait-word" className="font-medium text-wait">
          {WAIT_LABEL[item.kind] ?? item.kind}
        </span>
        {item.stage ? (
          <span data-kind="wait-stage" className="rounded-badge bg-hover px-1.5 py-[1px]">
            {stageLabel(item.stage)}
          </span>
        ) : null}
        <span className="ml-auto brand-type">
          {item.turn !== undefined && item.turn !== null ? '第 ' + item.turn + ' 轮' : ''}
        </span>
      </div>
      <div className="mt-0.5 truncate text-2 text-ink" title={item.title} data-kind="wait-title">
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
        {pick ? (
          <button
            type="button"
            data-action="wait-accept"
            disabled={accepting}
            onClick={() => void acceptOne(pick)}
            className="ml-auto rounded-btn bg-ink px-2 py-0.5 text-1 text-content transition-opacity hover:opacity-90 disabled:opacity-50"
            style={{ color: 'var(--content)' }}
          >
            接受
          </button>
        ) : null}
        {action ? (
          <button
            type="button"
            data-action="wait-open"
            onClick={() => void openWaitItem(item)}
            className={cn(
              'flex items-center gap-0.5 rounded-btn px-1.5 py-0.5 text-1 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2',
              pick ? '' : 'ml-auto',
            )}
          >
            {action} <span aria-hidden="true">›</span>
          </button>
        ) : null}
      </div>
    </article>
  );
}

function AcceptGroupHeader({ items }: { items: WaitItem[] }) {
  const selection = useWorkStore((state) => state.selection);
  const accepting = useWorkStore((state) => state.accepting);
  const selectAll = useWorkStore((state) => state.selectAllAccept);
  const clearSelection = useWorkStore((state) => state.clearSelection);
  const acceptSelected = useWorkStore((state) => state.acceptSelected);
  const count = Object.keys(selection).length;
  const picks = items.filter((item) => item.pick);
  return (
    <div className="ml-auto flex flex-wrap items-center justify-end gap-1 text-1">
      {picks.length ? (
        <button
          type="button"
          data-action="wait-select-all"
          disabled={accepting}
          onClick={selectAll}
          className="rounded-btn px-1.5 py-0.5 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2 disabled:opacity-50"
        >
          勾选全部 {picks.length} 项结果
        </button>
      ) : null}
      {count ? (
        <>
          <button
            type="button"
            data-action="wait-clear"
            disabled={accepting}
            onClick={clearSelection}
            className="rounded-btn px-1.5 py-0.5 text-ink-3 transition-colors hover:bg-hover hover:text-ink-2 disabled:opacity-50"
          >
            取消勾选
          </button>
          <button
            type="button"
            data-action="wait-accept-selected"
            disabled={accepting}
            onClick={() => void acceptSelected()}
            className="rounded-btn bg-ink px-2 py-0.5 text-content transition-opacity hover:opacity-90 disabled:opacity-50"
            style={{ color: 'var(--content)' }}
          >
            {accepting ? '正在接受…' : '接受选中的 ' + String(count) + ' 项'}
          </button>
        </>
      ) : null}
    </div>
  );
}

export function WaitList({ items, emptyText }: { items: WaitItem[]; emptyText?: string }) {
  const groups = waitGroups(items);
  const receipt = useWorkStore((state) => state.receipt);
  if (groups.length === 0 && !receipt) {
    return <p className="px-1 py-2 text-1 text-ink-3">{emptyText ?? '没有要你的事。'}</p>;
  }
  return (
    <div className="space-y-3" data-kind="wait-list">
      {receipt ? <AcceptReceipt receipt={receipt} /> : null}
      {groups.map((group) => (
        <section key={group.kind} data-kind="wait-group" data-wait-group={group.kind}>
          <header className="mb-1 flex flex-wrap items-baseline gap-1.5 px-0.5 text-1">
            <span className="text-wait">{group.label}</span>
            <span className="brand-type text-ink-3">{group.items.length}</span>
            {group.kind === 'accept' ? <AcceptGroupHeader items={group.items} /> : null}
          </header>
          <div className="space-y-1.5">
            {group.items.map((item, index) => (
              <WaitRow
                key={item.kind + ':' + (item.taskId ?? item.title) + ':' + String(index)}
                item={item}
              />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
