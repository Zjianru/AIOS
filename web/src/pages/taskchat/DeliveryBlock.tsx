/**
 * T058：任务页头部「交付记录」块；只画服务端派生的 DeliveryRecord，默认收起。
 * 读一次由 TaskChatScene 管；这里没有按钮会改任务。
 */
import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import type { DeliveryLine } from '@/data/generated/DeliveryLine';
import type { DeliveryRecord } from '@/data/generated/DeliveryRecord';
import type { DeliveryHistoryItem } from '@/data/generated/DeliveryHistoryItem';
import type { TaskReadGateCheck } from '@/data/generated/TaskReadGateCheck';
import { clockOf } from '@/lib/numbers';
import { cn } from '@/lib/utils';

const TONE: Record<string, string> = {
  done: 'text-done',
  wait: 'text-wait',
  run: 'text-run',
  plain: 'text-ink-2',
  muted: 'text-ink-3',
};

function LineView({ line, kind }: { line: DeliveryLine; kind?: string }) {
  return (
    <div className={cn('min-w-0', TONE[line.tone] ?? 'text-ink-2')} data-kind={kind}>
      {line.text}
      {line.at ? <span className="ml-1 text-ink-3">· {clockOf(line.at)}</span> : null}
    </div>
  );
}

function checkWord(check: TaskReadGateCheck): string {
  if (check.exitCode !== undefined && check.exitCode !== null) {
    return '退出码 ' + String(check.exitCode);
  }
  const words: Record<string, string> = {
    timed_out: '超时',
    spawn_failed: '起不来',
    stopped: '被停',
    skipped: '没跑',
  };
  return words[check.outcome] ?? '没有退出码';
}

function CheckRows({ checks }: { checks: TaskReadGateCheck[] }) {
  return (
    <div className="mt-1 space-y-0.5" data-kind="delivery-check-rows">
      {checks.map((check) => (
        <div key={check.index} className="flex min-w-0 items-center gap-2 text-1 leading-4" data-kind="delivery-check">
          <span className="brand-type w-4 shrink-0 text-right text-ink-3">{check.index + 1}</span>
          <code className="process-path min-w-0 flex-1 truncate text-ink-2" title={check.command}>
            {check.command}
          </code>
          <span className="shrink-0 text-ink-3">{checkWord(check)}</span>
          {check.durationMs > 0 ? (
            <span className="brand-type shrink-0 text-ink-3">
              {check.durationMs >= 1000
                ? (check.durationMs / 1000).toFixed(1) + 's'
                : check.durationMs + 'ms'}
            </span>
          ) : null}
        </div>
      ))}
    </div>
  );
}

function historyText(item: DeliveryHistoryItem): string {
  return (
    '第 ' +
    String(item.attempt) +
    ' 次结果 · 第 ' +
    String(item.version) +
    ' 版要求 · ' +
    clockOf(item.at) +
    ' · ' +
    (item.accepted ? '你接受过' : item.word)
  );
}

export function DeliveryBlock({ delivery }: { delivery: DeliveryRecord | null }) {
  const [open, setOpen] = useState(false);
  const [checksOpen, setChecksOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  if (!delivery) return null;
  const { what, basis } = delivery;
  return (
    <div className="mt-1" data-kind="task-delivery">
      <button
        type="button"
        data-kind="delivery-toggle"
        onClick={() => setOpen((value) => !value)}
        className="flex items-center gap-1 text-2 text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight className={cn('size-3 transition-transform', open && 'rotate-90')} />
        交付记录 · 第 {delivery.attempt} 次结果 · 第 {delivery.version} 版要求 ·
        <span className={TONE[delivery.word.tone] ?? 'text-ink-2'} data-kind="delivery-word">
          {delivery.word.text}
        </span>
      </button>
      {open ? (
        <div className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 border-l border-line pl-3 text-2 leading-5">
          <span className="text-ink-3">交了什么</span>
          <div className="min-w-0 text-ink-2" data-kind="delivery-what">
            <div className="line-clamp-3">{what.summary}</div>
            <div className="text-ink-3">
              第 {what.attempt} 次结果 · {clockOf(what.at)}
            </div>
            {what.changed === null ? (
              <div className="text-ink-3">调查任务：没有改动清单</div>
            ) : (
              <div className="mt-0.5" data-kind="delivery-changed">
                {what.changed.length ? (
                  <span>
                    改了 {what.changed.length} 个文件：
                    {what.changed.map((path) => (
                      <span key={path} className="process-path mr-2">
                        {path}
                      </span>
                    ))}
                  </span>
                ) : (
                  <span className="text-ink-3">没有记录到改过的文件</span>
                )}
                {what.bash ? (
                  <div className="text-wait">开了 bash：清单只含写、改工具做的改动</div>
                ) : null}
              </div>
            )}
            <div className="text-ink-3">用途：没有记录</div>
          </div>

          <span className="text-ink-3">依据什么</span>
          <div className="min-w-0 space-y-0.5" data-kind="delivery-basis">
            <LineView line={basis.version} />
            <div className="flex min-w-0 gap-2">
              <span className="shrink-0 text-ink-3">判定</span>
              <LineView line={basis.verdict} kind="delivery-verdict" />
            </div>
            {basis.note ? <div className="text-ink-2">你写的：「{basis.note}」</div> : null}
            <div className="flex min-w-0 gap-2">
              <span className="shrink-0 text-ink-3">检查记录</span>
              <div className="min-w-0">
                <LineView line={basis.record} kind="delivery-record" />
                {basis.checks ? (
                  <>
                    <button
                      type="button"
                      data-kind="delivery-checks-toggle"
                      onClick={() => setChecksOpen((value) => !value)}
                      className="text-1 text-ink-3 transition-colors hover:text-ink-2"
                    >
                      {checksOpen ? '收起命令' : '看每条命令'}
                    </button>
                    {checksOpen ? <CheckRows checks={basis.checks.checks} /> : null}
                  </>
                ) : null}
              </div>
            </div>
          </div>

          <span className="text-ink-3">还缺什么</span>
          <div className="min-w-0 space-y-0.5" data-kind="delivery-missing">
            {delivery.missing.map((line) => (
              <LineView key={line.text} line={line} />
            ))}
          </div>

          <span className="text-ink-3">交到了哪</span>
          <LineView line={delivery.where} kind="delivery-where" />

          <span className="text-ink-3">接受了吗</span>
          <div className="min-w-0 space-y-0.5" data-kind="delivery-accepted">
            <LineView line={delivery.accepted.state} />
            {delivery.accepted.revisions?.map((line) => (
              <LineView key={line.text} line={line} kind="delivery-version" />
            ))}
          </div>

          {delivery.history.length ? (
            <>
              <span />
              <div className="min-w-0">
                <button
                  type="button"
                  data-kind="delivery-history-toggle"
                  onClick={() => setHistoryOpen((value) => !value)}
                  className="text-1 text-ink-3 transition-colors hover:text-ink-2"
                >
                  {historyOpen ? '收起之前的结果' : '之前的结果 ' + String(delivery.history.length) + ' 份'}
                </button>
                {historyOpen ? (
                  <div className="mt-1 space-y-0.5" data-kind="delivery-history">
                    {delivery.history.map((item) => (
                      <div key={item.attempt} className="text-1 leading-4 text-ink-3">
                        {historyText(item)}
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
