/**
 * 确认卡的视图：一条标题、一段说明、几个单选选项（每项带服务端填的事实行）、
 * 「暂不」「确认」两个按钮。点过以后收成回执，写着选了哪一项和结果。
 *
 * 卡片不知道具体动作：按钮只有 confirm / dismiss 两个，选项 id 原样交给程序操作。
 */

import { useState } from 'react';
import { Check, CircleCheck, CircleHelp, TriangleAlert } from 'lucide-react';
import type { CardViewProps, ConfirmFactSlot, ConfirmOptionSlot, ConfirmSlots } from './model';
import { cardTone } from './model';
import { BodyText } from './BodyText';
import { cn } from '@/lib/utils';

function FactRows({ facts }: { facts: ConfirmFactSlot[] }) {
  if (facts.length === 0) return null;
  return (
    <ul className="mt-1 space-y-0.5" data-kind="confirm-facts">
      {facts.map((fact, index) => (
        <li
          key={fact.name + '-' + String(index)}
          data-kind="confirm-fact"
          className="flex min-w-0 gap-2 text-1 leading-4"
        >
          <span className="shrink-0 text-ink-3">{fact.name}</span>
          <span className="min-w-0 text-ink-2">{fact.value}</span>
        </li>
      ))}
    </ul>
  );
}

function OptionRow({
  option,
  selected,
  onSelect,
}: {
  option: ConfirmOptionSlot;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <div
      role="radio"
      aria-checked={selected}
      tabIndex={0}
      data-action="card-option"
      data-option-id={option.id}
      onClick={onSelect}
      onKeyDown={(event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        onSelect();
      }}
      className={cn(
        'flex w-full cursor-pointer items-start gap-2.5 rounded-btn border px-2.5 py-2 text-left transition-colors',
        selected ? 'border-ink-3/70 bg-hover' : 'border-line hover:bg-hover',
      )}
    >
      <span
        className={cn(
          'mt-[3px] grid size-3.5 shrink-0 place-items-center rounded-full border',
          selected ? 'border-ink bg-ink text-content' : 'border-line-2',
        )}
      >
        {selected ? <i className="size-1.5 rounded-full bg-content" /> : null}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-3 leading-5 text-ink">{option.label}</span>
        {option.description ? (
          <BodyText slot={option.description} className="text-2 leading-5 text-ink-3" />
        ) : null}
        <FactRows facts={option.facts} />
      </span>
    </div>
  );
}

function PendingCard({ envelope, slots, host }: CardViewProps<ConfirmSlots>) {
  // 单选：只有一个选项时它就是选中的那个，用户只需要确认或暂不。
  const [selected, setSelected] = useState<string | null>(() =>
    slots.options.length === 1 ? (slots.options[0]?.id ?? null) : null,
  );
  const tone = cardTone(envelope.role, envelope.state);
  return (
    <div
      className={cn('mt-2 rounded-card border p-3', tone.border, tone.background)}
      data-kind="confirm-card"
      data-card-id={envelope.id}
      data-card-type={envelope.type}
      data-role={envelope.role}
      data-state={envelope.state}
    >
      <div className={cn('flex items-center gap-1.5 text-2', tone.text)}>
        <CircleHelp className="size-3.5" />
        <span>Kind 想让你决定一件事</span>
        <span className="ml-auto text-1 text-ink-3">点了才会生效</span>
      </div>
      {slots.title ? <div className="mt-3 text-3 leading-5 text-ink">{slots.title}</div> : null}
      <BodyText slot={slots.brief} className="mt-1 text-2 leading-5 text-ink-2" />
      <div className="mt-2 space-y-1" role="radiogroup">
        {slots.options.map((option) => (
          <OptionRow
            key={option.id}
            option={option}
            selected={selected === option.id}
            onSelect={() => setSelected(option.id)}
          />
        ))}
      </div>
      <div className="mt-3 flex items-center gap-2">
        <span className="text-1 text-ink-3">{selected ? '选好了' : '先选一项'}</span>
        <button
          type="button"
          data-action="card-dismiss"
          onClick={() => host.dismissCard?.()}
          className="ml-auto rounded-btn px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
        >
          暂不
        </button>
        <button
          type="button"
          data-action="card-confirm"
          disabled={!selected}
          onClick={() => selected && host.confirmCard?.(selected)}
          className={cn(
            'rounded-btn px-3 py-1 text-2 transition-opacity',
            selected ? 'bg-ink text-content hover:opacity-90' : 'cursor-not-allowed bg-hover text-ink-3',
          )}
        >
          确认
        </button>
      </div>
    </div>
  );
}

function ReceiptCard({ envelope, slots }: CardViewProps<ConfirmSlots>) {
  const tone = cardTone(envelope.role, envelope.state);
  const failed = envelope.state === 'failed';
  return (
    <div
      className={cn('mt-2 border-l pl-3 text-2 leading-5', tone.border)}
      data-kind="confirm-card"
      data-card-id={envelope.id}
      data-card-type={envelope.type}
      data-role={envelope.role}
      data-state={envelope.state}
      data-receipt={envelope.receipt?.outcome ?? ''}
    >
      <div className={cn('flex items-center gap-1.5', tone.text)} data-kind="confirm-receipt">
        {failed ? (
          <TriangleAlert className="size-3.5" />
        ) : envelope.receipt?.outcome === 'success' ? (
          <CircleCheck className="size-3.5" />
        ) : (
          <Check className="size-3.5" />
        )}
        {envelope.receipt?.summary}
      </div>
      {slots.title ? <div className="mt-1 text-1 text-ink-3">{slots.title}</div> : null}
    </div>
  );
}

/** 确认卡：没点时是一张带选项的卡；点过、被替换、失败都收成回执。 */
export function ConfirmCardView(props: CardViewProps<ConfirmSlots>) {
  if (props.envelope.state === 'pending') return <PendingCard {...props} />;
  return <ReceiptCard {...props} />;
}
