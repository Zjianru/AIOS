//! 评审：门判定与候选轮次。

import { clockOf } from '@/lib/numbers';
import type { TaskReadGate } from '@/data/generated/TaskReadGate';

/** 头部「任务」菜单（D81）：立即停、收尾停；任务没在跑时两项都灰。 */
/** F4 A：里程碑门任务页显示每次判定与历次核查 Attempt。 */
export function ReviewSection({ gate }: { gate: TaskReadGate }) {
  const verdicts = gate.verdicts ?? [];
  const attempts = gate.attempts ?? [];
  const wordOf = (conclusion: string) =>
    conclusion === 'pass' ? '通过' : conclusion === 'needs_fix' ? '需修正' : '未确定';
 // 协议里的时刻是毫秒；显示统一走 clockOf（秒）。
  const timeOf = (at: number) => clockOf(at / 1000);
  return (
    <div className="mt-3 border-t border-line pt-3" data-kind="review-section">
      <div className="text-2 font-medium text-ink">核查判定</div>
      {verdicts.length === 0 ? (
        <div className="mt-1 text-2 text-ink-3" data-kind="review-empty">
          还没判
        </div>
      ) : (
        <div className="mt-1 space-y-1.5" data-kind="review-verdicts">
          {verdicts.map((verdict, index) => (
            <div
              key={String(index)}
              className="border-l-2 border-line-2 pl-2.5 text-2 leading-5"
              data-kind="review-verdict"
            >
              <div className="flex flex-wrap items-center gap-x-2 text-ink-2">
                <span className="text-ink">
                  {verdict.candidateOrdinal ? '候选第 ' + verdict.candidateOrdinal + ' 次' : '候选'}
                </span>
                <span className={verdict.conclusion === 'pass' ? 'text-done' : 'text-wait'}>
                  {wordOf(verdict.conclusion)}
                </span>
                <span className="text-1 text-ink-3">{timeOf(verdict.at)}</span>
                <span className="text-1 text-ink-3">
                  {verdict.judgedBy === 'user' ? '你判的' : '程序判的'}
                </span>
              </div>
              {verdict.note ? (
                <div className="mt-1 whitespace-pre-wrap break-words text-ink-2" data-kind="review-note">
                  <span className="text-1 text-ink-3">用户原话 </span>
                  {verdict.note}
                </div>
              ) : null}
            </div>
          ))}
        </div>
      )}
      <div className="mt-2 text-1 text-ink-3">历次核查 Attempt</div>
      {attempts.length === 0 ? (
        <div className="mt-0.5 text-2 text-ink-3" data-kind="review-attempts-empty">
          还没有核查记录
        </div>
      ) : (
        <ul className="mt-0.5 space-y-1" data-kind="review-attempts">
          {attempts.map((attempt, index) => (
            <li
              key={String(attempt.attempt) + '-' + String(index)}
              className="text-2 leading-5"
              data-kind="review-attempt"
            >
              <span className="text-ink-2">第 {attempt.attempt} 次</span>
              <span className="ml-2 text-1 text-ink-3">{timeOf(attempt.at)}</span>
              <div className="line-clamp-2 text-ink-2">{attempt.summary}</div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
