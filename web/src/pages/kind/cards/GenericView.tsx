/**
 * 通用卡（T017.2）：认不出的信封退回这里。只画标题和正文，不报错。
 * T017.3：不认识的工具展开后也是通用卡——带输入 / 输出两段时照 T013.5 的样子画。
 */

import type { BodySlot, CardViewProps, GenericSlots } from './model';
import { cardTone } from './model';
import { BodyPre, BodyText } from './BodyText';
import { Shell } from './ToolViews';
import { cn } from '@/lib/utils';

/** 通用工具卡的一段：输入 / 输出。 */
function Section({ caption, slot, tone }: { caption: string; slot: BodySlot; tone?: string }) {
  return (
    <div className="px-4 py-3">
      <div className="mb-1 text-1 text-ink-3">{caption}</div>
      <BodyPre
        slot={slot}
        className={cn(
          'max-h-[150px] overflow-auto whitespace-pre-wrap break-all font-mono text-2 leading-[18px]',
          tone ?? 'text-ink-2',
        )}
      />
    </div>
  );
}

export function GenericCardView({ envelope, slots }: CardViewProps<GenericSlots>) {
  const tone = cardTone(envelope.role, envelope.state);
  // 工具卡：不认识的工具、或者认识的工具取不出槽位；输入 / 输出照旧。
  if (slots.input || slots.output) {
    return (
      <Shell>
        {slots.input ? <Section caption="输入" slot={slots.input} /> : null}
        {slots.output ? (
          <div className="border-t-[0.5px] border-line-2">
            <Section caption="输出" slot={slots.output} tone={envelope.state === 'failed' ? 'text-fail' : undefined} />
          </div>
        ) : null}
      </Shell>
    );
  }
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
