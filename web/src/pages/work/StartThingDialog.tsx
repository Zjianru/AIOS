/**
 * 「交给 Kind 一件事」（§四.4 三档露脸在工作页的落点）。
 *
 * 开始一件事时，Kind 要在对话里说明这次按哪档做：
 * 轻档「我直接做了」/ 普通档「先给任务图请你确认」/ 配对档「几个人并行、谁来验」。
 * 原型不真的起任务：点「开工」给一句提示。
 */

import { useState } from 'react';
import { toast } from 'sonner';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { cn } from '@/lib/utils';

const TIERS = [
  { id: 'light', name: '轻档', when: '小事、一句话能说清', say: '我直接做了，做完给你看结果。' },
  { id: 'normal', name: '普通档', when: '多几站、要你点一次头', say: '我先给你一张任务图，你确认了再开工。' },
  { id: 'pair', name: '配对档', when: '要求高、要有人复核', say: '两个人并行：一个人做、另一个人验，最后我核对。' },
];

export function StartThingDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [text, setText] = useState('');
  const [tier, setTier] = useState('normal');
  const say = TIERS.find((item) => item.id === tier)?.say ?? '';

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[560px]">
        <DialogTitle className="text-4 font-medium text-ink">交给 Kind 一件事</DialogTitle>
        <DialogDescription className="text-2 text-ink-3">说清楚要什么结果就行；Kind 按选中的档位开工。</DialogDescription>

        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          rows={3}
          placeholder="例如：把这周的会议纪要整理成一页"
          className="mt-3 w-full resize-none rounded-btn border border-line-2 bg-pane px-2.5 py-2 text-3 text-ink outline-none placeholder:text-ink-3 focus:border-ink-3"
        />

        <div className="mt-3 text-1 text-ink-3">这次按哪档做</div>
        <div className="mt-1.5 grid gap-1.5">
          {TIERS.map((item) => (
            <button
              key={item.id}
              type="button"
              onClick={() => setTier(item.id)}
              className={cn(
                'flex items-start gap-2 rounded-card border px-2.5 py-2 text-left transition-colors',
                item.id === tier ? 'border-ink-3 bg-hover' : 'border-line hover:bg-hover',
              )}
            >
              <span
                className={cn(
                  'mt-1 size-3 shrink-0 rounded-full border',
                  item.id === tier ? 'border-ink-3 bg-ink-3' : 'border-line-2',
                )}
              />
              <span className="min-w-0">
                <span className="text-2 text-ink">{item.name}</span>
                <span className="ml-2 text-1 text-ink-3">{item.when}</span>
                <span className="mt-0.5 block text-2 leading-5 text-ink-2">{item.say}</span>
              </span>
            </button>
          ))}
        </div>

        <div className="mt-3 rounded-card bg-pane/60 p-2.5 text-2 leading-5 text-ink-2">
          <span className="text-1 text-ink-3">Kind 会先在对话里说：</span>
          <br />
          {say}
        </div>

        <div className="mt-3 flex justify-end gap-2">
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            className="rounded-btn border border-line-2 px-3 py-1.5 text-2 text-ink-2 transition-colors hover:bg-hover"
          >
            取消
          </button>
          <button
            type="button"
            disabled={!text.trim()}
            onClick={() => {
              onOpenChange(false);
              toast('（原型）已开工：' + (TIERS.find((item) => item.id === tier)?.name ?? '') + '。真起来以后，这里会换成 Kind 的对话记录。');
            }}
            className={cn(
              'rounded-btn px-3 py-1.5 text-2 transition-opacity',
              text.trim() ? 'bg-ink text-content hover:opacity-90' : 'cursor-not-allowed bg-hover text-ink-3',
            )}
          >
            开工
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
