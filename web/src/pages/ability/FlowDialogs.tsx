import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import {
  ArrowLeft,
  Download,
  FilePlus2,
  History,
  LayoutTemplate,
  MessageSquare,
} from 'lucide-react';
import {
  DISTILL_OPTIONS,
  IMPORT_PREVIEW,
  NEW_FLOW_SOURCES,
  SAY_DEFAULT,
} from '@/data/flows';
import type { NewFlowSource, NewFlowTemplate } from '@/data/flows';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';

export type NewFlowChoice =
  | 'blank'
  | 'say'
  | 'weekly'
  | 'writing'
  | 'podcast'
  | 'content'
  | 'unfix'
  | 'tpl-builtin-light'
  | 'tpl-builtin-normal'
  | 'tpl-builtin-pair';

interface NewFlowDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreate: (choice: NewFlowChoice) => void;
  /** 模板来源的样例草稿，由上层从 api 取到再传下来。 */
  templates: NewFlowTemplate[];
}

const SOURCE_ICON: Record<NewFlowSource['id'], typeof FilePlus2> = {
  blank: FilePlus2,
  say: MessageSquare,
  distil: History,
  template: LayoutTemplate,
  import: Download,
};

const primaryButton =
  'h-8 rounded-btn bg-ink px-3 text-2 font-semibold text-content transition-opacity hover:opacity-90 disabled:opacity-50';
const ghostButton =
  'h-8 rounded-btn border border-line-2 px-3 text-2 text-ink-2 transition-colors hover:bg-hover hover:text-ink';
const optionRow =
  'grid w-full grid-cols-[1fr_auto] gap-x-3 gap-y-0.5 rounded-card border border-line-2 px-3 py-2.5 text-left transition-colors hover:bg-hover';

export function NewFlowDialog({ open, onOpenChange, onCreate, templates }: NewFlowDialogProps) {
  const [step, setStep] = useState<'pick' | 'say' | 'distil' | 'template' | 'import'>('pick');
  const [busy, setBusy] = useState(false);
  const [peek, setPeek] = useState(false);
  const [sayText, setSayText] = useState(SAY_DEFAULT);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (open) return;
    setStep('pick');
    setBusy(false);
    setPeek(false);
    if (timer.current) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  }, [open]);

  useEffect(
    () => () => {
      if (timer.current) window.clearTimeout(timer.current);
    },
    [],
  );

  const createSay = () => {
    if (busy) return;
    setBusy(true);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setBusy(false);
      onCreate('say');
    }, 520);
  };

  const back = (
    <button type="button" onClick={() => setStep('pick')} className="inline-flex items-center gap-1 self-start text-2 text-ink-3 hover:text-ink">
      <ArrowLeft className="size-3.5" />
      换一种
    </button>
  );

  let body: ReactNode = null;
  if (step === 'pick') {
    body = (
      <div className="mt-4 grid grid-cols-5 gap-2.5">
        {NEW_FLOW_SOURCES.map((source) => {
          const Icon = SOURCE_ICON[source.id];
          return (
            <button
              key={source.id}
              type="button"
              onClick={() => {
                if (source.id === 'blank') {
                  onCreate('blank');
                  return;
                }
                setStep(source.id);
              }}
              className="flex flex-col items-start gap-1.5 rounded-card border border-line-2 px-3 py-3 text-left transition-colors hover:bg-hover"
            >
              <Icon className="size-5 text-ink-2" />
              <b className="text-3 font-semibold text-ink">{source.title}</b>
              <span className="text-1 leading-4 text-ink-3">{source.description}</span>
            </button>
          );
        })}
      </div>
    );
  } else if (step === 'say') {
    body = (
      <div className="mt-4 flex flex-col gap-3">
        {back}
        <textarea
          className="min-h-[76px] w-full resize-y rounded-card border border-line-2 bg-raised px-3 py-2 text-3 leading-6 text-ink outline-none focus:border-run"
          value={sayText}
          onChange={(event) => setSayText(event.target.value)}
          aria-label="跟 Kind 说一句"
        />
        <div className="flex justify-end">
          <button type="button" onClick={createSay} disabled={busy} className={primaryButton}>
            {busy ? 'Kind 在起草…' : '让 Kind 起草'}
          </button>
        </div>
      </div>
    );
  } else if (step === 'distil') {
    body = (
      <div className="mt-4 flex flex-col gap-3">
        {back}
        <p className="text-2 text-ink-3">这些事你做过不止一次。挑一件，Kind 把每次都一样的部分整理成流程，拿不准的会标出来让你补。</p>
        {DISTILL_OPTIONS.map((option) => (
          <button key={option.key} type="button" onClick={() => onCreate('weekly')} className={optionRow}>
            <b className="text-3 font-semibold text-ink">{option.title}</b>
            <span className="text-2 text-ink-3">{option.description}</span>
            <span className="col-start-2 row-span-2 row-start-1 self-center text-2 text-ink-3">提炼 →</span>
          </button>
        ))}
      </div>
    );
  } else if (step === 'template') {
    body = (
      <div className="mt-4 flex flex-col gap-3">
        {back}
        {templates.map((option) => (
          <button
            key={option.key}
            type="button"
            onClick={() => {
              if (option.key === 'content') onCreate('content');
              else if (option.key === 'writing') onCreate('writing');
              else if (option.key === 'unfix') onCreate('unfix');
              else if (option.key === 'builtin-light') onCreate('tpl-builtin-light');
              else if (option.key === 'builtin-normal') onCreate('tpl-builtin-normal');
              else if (option.key === 'builtin-pair') onCreate('tpl-builtin-pair');
            }}
            className={optionRow}
          >
            <b className="text-3 font-semibold text-ink">{option.title}</b>
            <span className="text-2 text-ink-3">{option.description}</span>
            <span className="col-start-2 row-span-2 row-start-1 self-center text-2 text-ink-3">复制一份 →</span>
          </button>
        ))}
      </div>
    );
  } else {
    body = (
      <div className="mt-4 flex flex-col gap-3">
        {back}
        <input
          className="h-8 w-full rounded-btn border border-line-2 bg-raised px-2 text-2 text-ink-3 outline-none"
          value={IMPORT_PREVIEW.link}
          readOnly
          aria-label="分享链接"
        />
        {peek ? (
          <div className="rounded-card bg-hover px-3 py-2.5">
            <div className="flex items-center gap-2">
              <b className="text-3 font-semibold text-ink">{IMPORT_PREVIEW.title}</b>
              <span className="rounded-badge border border-line-2 px-1 brand-type text-1 text-ink-2">
                {IMPORT_PREVIEW.version}
              </span>
            </div>
            <p className="mt-1 text-2 text-ink-2">{IMPORT_PREVIEW.summary}</p>
            <p className="mt-1 text-2 text-ink-2">{IMPORT_PREVIEW.detail}</p>
          </div>
        ) : null}
        <div className="flex justify-end gap-2">
          {!peek ? (
            <button type="button" onClick={() => setPeek(true)} className={ghostButton}>
              看看里面有什么
            </button>
          ) : (
            <button type="button" onClick={() => onCreate('podcast')} className={primaryButton}>
              导入为草稿
            </button>
          )}
        </div>
      </div>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-[calc(100vw-40px)] gap-0 rounded-[14px] border-line bg-content p-5 text-ink shadow-[var(--shadow-pop)] sm:max-w-[760px]"
        showCloseButton
      >
        <DialogTitle className="text-4 font-semibold text-ink">新建流程</DialogTitle>
        <DialogDescription className="mt-1 text-2 text-ink-2">
          五种开头，最后都是一份草稿，在编辑器里改完、检查过再保存。
        </DialogDescription>
        <div className="min-h-[212px]">{body}</div>
      </DialogContent>
    </Dialog>
  );
}
