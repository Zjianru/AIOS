/**
 * 提问卡的视图（T017.2）：从卡片信封的 slots 渲染，对话里的提问卡不再走旧的样例卡路径。
 *
 * - 等答时是一张卡：小标题（标签槽，坏了或超长就不显示）、问题、选项、提交、不答；
 * - 答过 / 不答 / 这一轮被停了：收成几行回执，不能再改；
 * - 正文太长：先显示前 600 字，点「展开全文」看全文。
 */

import { useEffect, useMemo, useState } from 'react';
import { Check, CircleHelp } from 'lucide-react';
import type {
  AnswerInput,
  AnswerSlot,
  CardViewProps,
  QuestionOptionSlot,
  QuestionSlot,
  QuestionSlots,
} from './model';
import { cardTone } from './model';
import { BodyText, previewOf } from './BodyText';
import { cn } from '@/lib/utils';

type Draft = Record<string, { selected: string[]; custom: string }>;

function answered(question: QuestionSlot, draft: Draft): boolean {
  const entry = draft[question.id];
  return Boolean(entry && (entry.selected.length > 0 || entry.custom.trim()));
}

function answerText(question: QuestionSlot, answers: AnswerSlot[]): string {
  const entry = answers.find((item) => item.id === question.id);
  if (!entry) return '—';
  return entry.selected.concat(entry.custom ? [entry.custom] : []).join('、');
}

function optionFolded(option: QuestionOptionSlot): boolean {
  return option.label.folded || Boolean(option.description?.folded);
}

function QuestionBlock({
  question,
  entry,
  index,
  total,
  onChange,
}: {
  question: QuestionSlot;
  entry: { selected: string[]; custom: string };
  index: number;
  total: number;
  onChange: (entry: { selected: string[]; custom: string }) => void;
}) {
  const multi = question.multiSelect;
  // 折起来的选项：先点开看全文，再点才选（选项本身是按钮，里面不能再套一个「展开」按钮）。
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const toggle = (label: string) => {
    if (multi) {
      const selected = entry.selected.includes(label)
        ? entry.selected.filter((item) => item !== label)
        : entry.selected.concat(label);
      onChange({ ...entry, selected });
    } else {
      onChange({ selected: [label], custom: '' });
    }
  };

  return (
    <fieldset className="min-w-0">
      <legend className="flex items-center gap-2 text-1 text-ink-3">
        {total > 1 ? (
          <span className="brand-type">
            {index + 1}/{total}
          </span>
        ) : null}
        {question.header ? <span>{question.header}</span> : null}
        {multi ? <span>· 可以选几个</span> : null}
      </legend>
      <BodyText slot={question.question} className="mt-1 text-3 leading-5 text-ink" />

      {question.options.length > 0 ? (
        <div className="mt-2 space-y-1" role={multi ? 'group' : 'radiogroup'}>
          {question.options.map((option, optionIndex) => {
            const on = entry.selected.includes(option.label.text);
            const folded = optionFolded(option);
            const open = expanded[optionIndex] === true;
            const showPreview = folded && !open;
            return (
              <button
                key={option.label.text}
                type="button"
                role={multi ? 'checkbox' : 'radio'}
                aria-checked={on}
                onClick={() => {
                  if (showPreview) {
                    setExpanded((current) => ({ ...current, [optionIndex]: true }));
                    return;
                  }
                  toggle(option.label.text);
                }}
                className={cn(
                  'flex w-full items-start gap-2.5 rounded-btn border px-2.5 py-2 text-left transition-colors',
                  on ? 'border-ink-3/70 bg-hover' : 'border-line hover:bg-hover',
                )}
              >
                <span
                  className={cn(
                    'mt-[3px] grid size-3.5 shrink-0 place-items-center border',
                    multi ? 'rounded-[3px]' : 'rounded-full',
                    on ? 'border-ink bg-ink text-content' : 'border-line-2',
                  )}
                >
                  {on ? (
                    multi ? (
                      <Check className="size-2.5" strokeWidth={3} />
                    ) : (
                      <i className="size-1.5 rounded-full bg-content" />
                    )
                  ) : null}
                </span>
                <span className="min-w-0">
                  <span className="block text-3 leading-5 text-ink">
                    {showPreview ? previewOf(option.label.text) : option.label.text}
                  </span>
                  {option.description && option.description.text ? (
                    <span className="block text-2 leading-5 text-ink-3">
                      {showPreview && option.description.folded
                        ? previewOf(option.description.text)
                        : option.description.text}
                    </span>
                  ) : null}
                  {showPreview ? <span className="block text-1 text-ink-3">展开全文</span> : null}
                </span>
              </button>
            );
          })}
          <label
            className={cn(
              'flex items-center gap-2.5 rounded-btn border px-2.5 py-1.5 transition-colors',
              entry.custom.trim() ? 'border-ink-3/70 bg-hover' : 'border-line',
            )}
          >
            <span className="shrink-0 text-2 text-ink-3">其他</span>
            <input
              value={entry.custom}
              onChange={(event) =>
                onChange({
                  selected: multi || !event.target.value.trim() ? entry.selected : [],
                  custom: event.target.value,
                })
              }
              placeholder="自己写"
              className="min-w-0 flex-1 bg-transparent text-3 leading-6 text-ink outline-none placeholder:text-ink-3"
            />
          </label>
        </div>
      ) : (
        <textarea
          rows={2}
          value={entry.custom}
          onChange={(event) => onChange({ selected: [], custom: event.target.value })}
          placeholder="写下你的回答"
          className="mt-2 w-full resize-none rounded-btn border border-line bg-transparent px-2.5 py-1.5 text-3 leading-5 text-ink outline-none transition-colors placeholder:text-ink-3 focus:border-ink-3/70"
        />
      )}
    </fieldset>
  );
}

function PendingCard({ envelope, slots, host }: CardViewProps<QuestionSlots>) {
  const questions = slots.questions;
  const [draft, setDraft] = useState<Draft>(() =>
    Object.fromEntries(questions.map((question) => [question.id, { selected: [], custom: '' }])),
  );
  // 问题卡可能分批到（question/asked 一条一题）：补齐新题的草稿，避免 undefined。
  useEffect(() => {
    setDraft((current) => {
      const missing = questions.filter((question) => !current[question.id]);
      if (missing.length === 0) return current;
      const next = { ...current };
      for (const question of missing) next[question.id] = { selected: [], custom: '' };
      return next;
    });
  }, [questions]);

  const missing = useMemo(
    () => questions.filter((question) => !answered(question, draft)).length,
    [questions, draft],
  );
  const tone = cardTone(envelope.role, envelope.state);

  const submit = () => {
    if (missing) return;
    const answers: AnswerInput[] = questions.map((question) => {
      const entry = draft[question.id] ?? { selected: [], custom: '' };
      return {
        id: question.id,
        selected: entry.selected,
        ...(entry.custom.trim() ? { custom: entry.custom.trim() } : {}),
      };
    });
    host.answer(answers);
  };

  return (
    <div
      className={cn('mt-2 rounded-card border p-3', tone.border, tone.background)}
      data-kind="question"
      data-card-id={envelope.id}
      data-card-type={envelope.type}
      data-role={envelope.role}
      data-state={envelope.state}
      onKeyDown={(event) => {
        if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
          event.preventDefault();
          submit();
        }
      }}
    >
      <div className={cn('flex items-center gap-1.5 text-2', tone.text)}>
        <CircleHelp className="size-3.5" />
        {questions.length > 1 ? 'Kind 想先问你 ' + String(questions.length) + ' 件事' : 'Kind 想先问你一件事'}
        <span className="ml-auto text-1 text-ink-3">它在等你，这一轮停在这里</span>
      </div>
      <div className="mt-3 space-y-4">
        {questions.map((question, index) => (
          <QuestionBlock
            key={question.id}
            question={question}
            entry={draft[question.id] ?? { selected: [], custom: '' }}
            index={index}
            total={questions.length}
            onChange={(entry) => setDraft((current) => ({ ...current, [question.id]: entry }))}
          />
        ))}
      </div>
      <div className="mt-4 flex items-center gap-2">
        <span className="text-1 text-ink-3">
          {missing ? '还有 ' + String(missing) + ' 题没答' : '⌘↩ 提交'}
        </span>
        <button
          type="button"
          data-action="question-dismiss"
          onClick={() => host.dismiss()}
          title="整批不答：Kind 会知道你没答，照常收尾"
          className="ml-auto rounded-btn px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
        >
          不答
        </button>
        <button
          type="button"
          data-action="question-answer"
          onClick={submit}
          aria-disabled={missing > 0}
          className={cn(
            'rounded-btn px-3 py-1 text-2 transition-opacity',
            missing ? 'cursor-not-allowed bg-hover text-ink-3' : 'bg-ink text-content hover:opacity-90',
          )}
        >
          提交回答
        </button>
      </div>
    </div>
  );
}

function ClosedCard({ envelope, slots }: CardViewProps<QuestionSlots>) {
  const tone = cardTone(envelope.role, envelope.state);
  return (
    <div
      className={cn('mt-2 border-l pl-3 text-2 leading-5', tone.border)}
      data-kind="question-closed"
      data-card-id={envelope.id}
      data-card-type={envelope.type}
      data-role={envelope.role}
      data-state={envelope.state}
    >
      <div className={cn('flex items-center gap-1.5', tone.text)}>
        <CircleHelp className="size-3.5" />
        {envelope.receipt?.summary}
      </div>
      {slots.answers.length > 0 ? (
        <ul className="mt-1 space-y-0.5">
          {slots.questions.map((question) => (
            <li key={question.id} className="text-ink-3">
              {question.question.text} <span className="text-ink-3">→</span>{' '}
              <span className="text-ink-2">{answerText(question, slots.answers)}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

/** 提问卡：等答一张卡，答过/不答/被停收成回执。 */
export function QuestionCardView(props: CardViewProps<QuestionSlots>) {
  if (props.envelope.state === 'pending') return <PendingCard {...props} />;
  return <ClosedCard {...props} />;
}
