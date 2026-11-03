/**
 * 模型提问卡（内核的 ask_user）：一次列出全部问题，整批作答。
 *
 * - 有选项的题：单选或多选；最后一行「其他」可以自己写（单选时写了就取代选项）。
 * - 没选项的题：直接写。
 * - 「提交回答」要每题都有答案；「不答」= 整批不答，模型收到「用户没答」后照常收尾。
 * - 答过 / 不答 / 这一轮被停了：收成几行，不再能改。
 */

import { useMemo, useState } from 'react';
import { Check, CircleHelp } from 'lucide-react';
import type { AskAnswer, AskQuestion } from '@/data/fixtures/kindLive';
import type { QuestionView } from './kindStore';
import { useKindStore } from './kindStore';
import { cn } from '@/lib/utils';

type Draft = Record<string, { selected: string[]; custom: string }>;

function answered(question: AskQuestion, draft: Draft): boolean {
  const entry = draft[question.id];
  return Boolean(entry && (entry.selected.length > 0 || entry.custom.trim()));
}

function QuestionBlock({ question, entry, onChange, index, total }: {
  question: AskQuestion;
  entry: { selected: string[]; custom: string };
  onChange: (entry: { selected: string[]; custom: string }) => void;
  index: number;
  total: number;
}) {
  const options = question.options ?? [];
  const multi = Boolean(question.multiSelect);
  const toggle = (label: string) => {
    if (multi) {
      const selected = entry.selected.includes(label) ? entry.selected.filter((item) => item !== label) : entry.selected.concat(label);
      onChange({ ...entry, selected });
    } else {
      onChange({ selected: [label], custom: '' });
    }
  };

  return (
    <fieldset className="min-w-0">
      <legend className="flex items-center gap-2 text-1 text-ink-3">
        {total > 1 ? <span className="brand-type">{index + 1}/{total}</span> : null}
        {question.header ? <span>{question.header}</span> : null}
        {multi ? <span>· 可以选几个</span> : null}
      </legend>
      <div className="mt-1 text-3 leading-5 text-ink">{question.question}</div>

      {options.length > 0 ? (
        <div className="mt-2 space-y-1" role={multi ? 'group' : 'radiogroup'}>
          {options.map((option) => {
            const on = entry.selected.includes(option.label);
            return (
              <button
                key={option.label}
                type="button"
                role={multi ? 'checkbox' : 'radio'}
                aria-checked={on}
                onClick={() => toggle(option.label)}
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
                  {on ? multi ? <Check className="size-2.5" strokeWidth={3} /> : <i className="size-1.5 rounded-full bg-content" /> : null}
                </span>
                <span className="min-w-0">
                  <span className="block text-3 leading-5 text-ink">{option.label}</span>
                  {option.description ? <span className="block text-2 leading-5 text-ink-3">{option.description}</span> : null}
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
                onChange({ selected: multi || !event.target.value.trim() ? entry.selected : [], custom: event.target.value })
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

function PendingCard({ turnId, questions }: { turnId: string; questions: AskQuestion[] }) {
  const answer = useKindStore((state) => state.answer);
  const dismiss = useKindStore((state) => state.dismiss);
  const [draft, setDraft] = useState<Draft>(() =>
    Object.fromEntries(questions.map((question) => [question.id, { selected: [], custom: '' }])),
  );
  const missing = useMemo(() => questions.filter((question) => !answered(question, draft)).length, [questions, draft]);

  const submit = () => {
    if (missing) return;
    const answers: AskAnswer[] = questions.map((question) => {
      const entry = draft[question.id];
      return { id: question.id, selected: entry.selected, ...(entry.custom.trim() ? { custom: entry.custom.trim() } : {}) };
    });
    answer(turnId, answers);
  };

  return (
    <div
      className="mt-2 rounded-card border border-wait/40 bg-wait-soft/40 p-3"
      data-kind="question"
      onKeyDown={(event) => {
        if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
          event.preventDefault();
          submit();
        }
      }}
    >
      <div className="flex items-center gap-1.5 text-2 text-wait">
        <CircleHelp className="size-3.5" />
        {questions.length > 1 ? `Kind 想先问你 ${questions.length} 件事` : 'Kind 想先问你一件事'}
        <span className="ml-auto text-1 text-ink-3">它在等你，这一轮停在这里</span>
      </div>
      <div className="mt-3 space-y-4">
        {questions.map((question, index) => (
          <QuestionBlock
            key={question.id}
            question={question}
            entry={draft[question.id]}
            index={index}
            total={questions.length}
            onChange={(entry) => setDraft((current) => ({ ...current, [question.id]: entry }))}
          />
        ))}
      </div>
      <div className="mt-4 flex items-center gap-2">
        <span className="text-1 text-ink-3">{missing ? `还有 ${missing} 题没答` : '⌘↩ 提交'}</span>
        <button
          type="button"
          onClick={() => dismiss(turnId)}
          title="整批不答：Kind 会知道你没答，照常收尾"
          className="ml-auto rounded-btn px-2.5 py-1 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink"
        >
          不答
        </button>
        <button
          type="button"
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

function answerText(question: AskQuestion, answers: AskAnswer[]): string {
  const entry = answers.find((item) => item.id === question.id);
  if (!entry) return '—';
  return entry.selected.concat(entry.custom ? [entry.custom] : []).join('、');
}

function ClosedCard({ question }: { question: Exclude<QuestionView, { status: 'pending' }> }) {
  const title =
    question.status === 'answered'
      ? `你回答了 Kind 的 ${question.questions.length} 个问题`
      : question.status === 'dismissed'
        ? '你没回答 Kind 的问题；它知道你没答，照常收尾'
        : '这一轮停下了，问题没再等回答';
  return (
    <div className="mt-2 border-l border-line pl-3 text-2 leading-5" data-kind="question-closed">
      <div className="flex items-center gap-1.5 text-ink-3">
        <CircleHelp className="size-3.5" />
        {title}
      </div>
      {question.status === 'answered' ? (
        <ul className="mt-1 space-y-0.5">
          {question.questions.map((item) => (
            <li key={item.id} className="text-ink-3">
              {item.question} <span className="text-ink-3">→</span> <span className="text-ink-2">{answerText(item, question.answers)}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

export function QuestionCard({ turnId, question }: { turnId: string; question: QuestionView }) {
  if (question.status === 'pending') return <PendingCard turnId={turnId} questions={question.questions} />;
  return <ClosedCard question={question} />;
}
