/**
 * 通用卡（T017.2）：认不出的信封退回这里。只画标题和正文，不报错。
 */

import type { CardViewProps, GenericSlots } from './model';
import { cardTone } from './model';
import { BodyText } from './BodyText';
import { cn } from '@/lib/utils';

export function GenericCardView({ envelope, slots, host }: CardViewProps<GenericSlots>) {
  void host;
  const tone = cardTone(envelope.role, envelope.state);
  return (
    <div
      className={cn('mt-2 rounded-card border p-3', tone.border, tone.background)}
      data-kind="card-generic"
      data-card-id={envelope.id}
      data-card-type={envelope.type}
      data-role={envelope.role}
      data-state={envelope.state}
    >
      {slots.title ? (
        <div className={cn('flex items-center gap-1.5 text-2', tone.text)}>{slots.title}</div>
      ) : null}
      <BodyText slot={slots.body} className="mt-1 whitespace-pre-wrap break-words text-2 leading-5 text-ink-2" />
    </div>
  );
}
