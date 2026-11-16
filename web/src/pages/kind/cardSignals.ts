//! 卡片信号：门判定/上游变了/接受资格与折卡后的视图信号。

import type { ServiceEvent } from '@/data/service';
import { parseTaskRelay } from '@/data/taskResult';
import { gateVerdictOf, upstreamChangedOf, userRpcIdOf } from './turnViews';
import type { TurnViewData } from './turnViews';

/** ：门判定 relay 回填被把关任务卡状态词（live 和刷新共用；按任务 id 记最近一次）。 */
export let gateVerdictByTask: ReadonlyMap<
  string,
  { conclusion: 'pass' | 'needs_fix'; candidateReport: string }
> = new Map();


/** ：上游变了的下游（upstream_changed relay；交出新 ready 报告时清除）。 */
export let upstreamChangedByTask: ReadonlySet<string> = new Set();


export function rememberCardSignals(events: readonly ServiceEvent[]): void {
  let verdicts = gateVerdictByTask;
  let changed = upstreamChangedByTask;
  for (const event of events) {
    if (event.type !== 'user/message') continue;
    const message = event.data?.message;
    const routed = gateVerdictOf(message);
    if (routed) {
      const existing = verdicts.get(routed.taskId);
      if (
        existing?.conclusion !== routed.conclusion ||
        existing?.candidateReport !== routed.candidateReport
      ) {
        verdicts = new Map(verdicts).set(routed.taskId, {
          conclusion: routed.conclusion,
          candidateReport: routed.candidateReport,
        });
      }
    }
    const rpcId = userRpcIdOf(message);
    if (rpcId?.startsWith('upstream-changed:')) {
      const downstream = upstreamChangedOf(message);
      if (downstream && !changed.has(downstream.downstreamId)) {
        changed = new Set(changed).add(downstream.downstreamId);
      }
      continue;
    }
    const relay = parseTaskRelay(message, 0);
    if (relay && changed.has(relay.taskId)) {
      const without = new Set(changed);
      without.delete(relay.taskId);
      changed = without;
    }
  }
  gateVerdictByTask = verdicts;
  upstreamChangedByTask = changed;
}


export function applyCardSignals(views: TurnViewData[]): TurnViewData[] {
  if (gateVerdictByTask.size === 0 && upstreamChangedByTask.size === 0) return views;
  return views.map((view) => {
    if (
      !view.taskResults.some(
        (fact) => gateVerdictByTask.has(fact.taskId) || upstreamChangedByTask.has(fact.taskId),
      )
    ) {
      return view;
    }
    return {
      ...view,
      taskResults: view.taskResults.map((fact) => {
        const verdict = gateVerdictByTask.get(fact.taskId);
        const nextVerdict =
          verdict && fact.reportId === verdict.candidateReport ? verdict.conclusion : undefined;
        const upstreamChanged = upstreamChangedByTask.has(fact.taskId);
        if (
          fact.gateVerdict === nextVerdict &&
          (fact.upstreamChanged === true) === upstreamChanged
        ) {
          return fact;
        }
        return {
          ...fact,
          gateVerdict: nextVerdict,
          ...(upstreamChanged ? { upstreamChanged: true } : {}),
        };
      }),
    };
  });
}


/**
 * ：结果卡上的「接受」「要求修改」由会话事实派生：只给每个任务最后一张结果卡，
 * 且它 ready、未接受；有门的要门已判通过（门任务自己的卡不出）。
 */
/** F1：每个任务「确认改要求」回执的 seq；比它早的结果卡已经是旧版本。 */
export function requirementChangeSeqs(events: readonly ServiceEvent[]): Map<string, number> {
  const seqs = new Map<string, number>();
  for (const event of events) {
    if (event.type !== 'card/decision') continue;
    const decision = event.data as unknown as {
      action?: unknown;
      phase?: unknown;
      data?: { result?: { task_id?: unknown } };
    };
    if (decision?.action !== 'update_task_requirement' || decision.phase !== 'confirmed') continue;
    const taskId = decision.data?.result?.task_id;
    if (typeof taskId !== 'string' || taskId === '') continue;
    if (typeof event.seq !== 'number') continue;
 // 取**最新**一次回执：第二次改要求时，上一版的卡早于新回执，要一起收起来。
    const current = seqs.get(taskId);
    if (current === undefined || event.seq > current) seqs.set(taskId, event.seq);
  }
  return seqs;
}


export function applyAcceptEligibility(views: TurnViewData[], events: readonly ServiceEvent[]): TurnViewData[] {
  const lastCard = new Map<string, string>();
  const changedSeqs = requirementChangeSeqs(events);
  for (const view of views) {
    for (const fact of view.taskResults) {
      if (fact.id.startsWith('task-report:')) lastCard.set(fact.taskId, fact.id);
    }
  }
  let changedAny = false;
  const next = views.map((view) => {
    let changed = false;
    const taskResults = view.taskResults.map((fact) => {
      if (!fact.id.startsWith('task-report:')) return fact;
      const gateSelf = fact.gate?.state === 'pending_verdict';
      const accepted =
        fact.decision?.phase === 'confirmed' &&
        !(gateSelf && fact.decision.data?.conclusion !== undefined);
      const gatePassed = fact.gate?.state !== 'waiting' || fact.gateVerdict === 'pass';
 // F1：确认改要求之后、新版本结果卡出来之前，旧版本的结果卡已经是旧卡：
 // 接受和「要求修改」一起收起来（刷新按同一条回执重建）。
      const changedSeq = changedSeqs.get(fact.taskId);
      const staleAfterChange =
        fact.seq !== undefined && changedSeq !== undefined && fact.seq < changedSeq;
      const acceptable =
        lastCard.get(fact.taskId) === fact.id &&
        fact.state === 'ready' &&
        !gateSelf &&
        gatePassed &&
        !accepted &&
        !staleAfterChange;
      if ((fact.acceptable === true) === acceptable) return fact;
      changed = true;
      return { ...fact, acceptable };
    });
    if (!changed) return view;
    changedAny = true;
    return { ...view, taskResults };
  });
  return changedAny ? next : views;
}
