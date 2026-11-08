/**
 * 卡片入口（T017.2）：页面只把信封交给它，它查注册表渲染。
 * 注册表每一项由纯数据的 cardDefinitions（schema / from / fallback）和这里的视图合成。
 */

import type { ReactElement } from 'react';
import type { CardDefinition, CardEnvelope, CardHost, CardType, CardViewProps } from './model';
import { cardDefinitions, normalizeEnvelope, resolveSlots } from './model';
import { GenericCardView } from './GenericView';
import { QuestionCardView } from './QuestionView';

interface RegisteredCard {
  resolve: (slots: unknown) => unknown;
  view: (props: { envelope: CardEnvelope; slots: unknown; host: CardHost }) => ReactElement;
}

/**
 * 把纯数据的注册项和一个视图合成一格。
 * 登记时 schema 的输出类型和视图的 slots 类型由类型参数对齐；表里擦成 unknown，
 * 进视图之前由 resolve() 再校验一次（字段坏了只是退回默认值，不整卡报错）。
 */
function registered<Fact, Slots>(
  definition: CardDefinition<Fact, Slots>,
  View: (props: CardViewProps<Slots>) => ReactElement,
): RegisteredCard {
  return {
    resolve: (slots) => resolveSlots(definition, slots),
    view: (props) => <View envelope={props.envelope} slots={props.slots as Slots} host={props.host} />,
  };
}

const registry: Partial<Record<CardType, RegisteredCard>> = {
  question: registered(cardDefinitions.question, QuestionCardView),
  generic: registered(cardDefinitions.generic, GenericCardView),
};

/**
 * 唯一的卡片入口：给它信封（可以是不合法的），它查表选视图。
 * 信封不合法（类型不认识、没有 id）整卡退回 generic；不抛异常。
 */
export function Card({ envelope, host }: { envelope: unknown; host: CardHost }): ReactElement | null {
  const resolved = normalizeEnvelope(envelope);
  const entry = registry[resolved.type] ?? registry.generic;
  if (!entry) return null;
  return entry.view({ envelope: resolved, slots: entry.resolve(resolved.slots), host });
}
