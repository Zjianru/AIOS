//! 场景 -> 回放脚本/路由文件的映射；宿主只认这里产出的文件名。

import type { FixtureOptions } from './options';

export interface ReplaySelection {
  file: string;
  override?: string;
  paceMs?: number;
}


/** ：场景 -> 录好的会话日志 + 覆盖文件（手写 SSE 已删除）。 */
export function replaySelection(options: FixtureOptions): ReplaySelection {
  if (options.failure === 'credential') return { file: 'text', override: 'failure-credential' };
  if (options.failure === 'model') return { file: 'text', override: 'failure-model' };
  if (options.failure === 'retry-then-succeed') return { file: 'text', override: 'failure-retry' };
  if (options.failure === 'unknown') return { file: 'text', override: 'failure-unknown' };
  if (options.realTurn) return { file: 'real', paceMs: 20 };
  if (options.usageTurn) return { file: 'usage', paceMs: 400 };
  if (options.tinyPercent) return { file: 'usage-tiny', paceMs: 400 };
  if (options.usageReadOnly) return { file: 'usage-read-only', paceMs: 400 };
  if (options.questionDegrade) return { file: 'question-degrade' };
  if (options.askTurn) return { file: 'ask', paceMs: options.askTurnPaceMs };
  if (options.toolTurn) return { file: 'tool' };
 // ：工具前说明 → read hello.txt → 最终答案；pace 150 让实时阶段能被观察到。
  if (options.pretextTurn) return { file: 't023-pretext', paceMs: 150 };
 // ：同一轮放慢到 400ms/帧，设置页往返后还有一段实时流可观察。
  if (options.pretextSlowTurn) return { file: 't023-pretext-slow', paceMs: 400 };
 // F5：内容超过视口的长流式回答，用来在流式追加中验证滚轮不被钉回底部。
  if (options.t059LongStream) return { file: 't059-long-stream', paceMs: 40 };
 // 同一句原话再记：两轮都是 memory_remember（第二轮用户消息 seq 22）。
  if (options.memoryAgain) return { file: 't061-memory-again', paceMs: 10 };
  if (options.t062MemoryCard) return { file: 't062-memory-tool-card', paceMs: 10 };
 // 记忆页联动的假模型脚本：记住/记一笔/5 个回合/做梦 apply+总结。
  if (options.memoryFlow) {
    if (options.memoryLiveCard) return { file: 't060-memory-live-card', paceMs: 10 };
    return options.memoryFailDream
      ? { file: 't060-memory', override: 't060-memory-fail-dream', paceMs: 10 }
      : { file: 't060-memory', paceMs: 10 };
  }
  if (options.codeBlock) return { file: 'tool-code' };
  if (options.toolCards) return { file: 'tool-cards' };
 // 只保留 Kind 现有工具（read/glob/grep）： 收窄后 Kind 没有 write/edit/bash。
  if (options.t013Interrupted) return { file: 'tool-interrupted-legal', paceMs: 800 };
  if (options.canvas) {
 // ：streaming 把 canvas 的参数拆成几段慢慢流（页面画「正在画 · 标题」）；
 // errors 是三个错误触发按钮的画布。
    if (options.canvas === 'streaming') return { file: 'canvas-streaming', paceMs: 120 };
    return { file: options.canvas === 'interactive' ? 'canvas' : 'canvas-' + options.canvas };
  }
  if (options.localFilePath) return { file: 'local-file-text' };
  if (options.highUsage) return { file: 'high-usage' };
  if (options.t014Query) return { file: 't014-query' };
  if (options.t014Confirm) return { file: 't014-confirm' };
  if (options.t014Mask) return { file: 't014-mask' };
  if (options.t014Web) return { file: 't014-web' };
  if (options.t014Search) return { file: 't014-search' };
  if (options.t014Fetch) return { file: 't014-fetch' };
  return {
    file: 'text',
    paceMs: options.slowMs ? Math.max(1, Math.round(options.slowMs / 6)) : undefined,
  };
}


/** ：重启接续夹具的初始路由文件（不含重启那一轮；测试拿到任务 id 后再生成完整路由）。 */
export function restartRouteFile(options: FixtureOptions): string | null {
  if (!options.t029Restart) return null;
  return 't029-routes-' + options.t029Restart + '.json';
}


/** ：双会话回放用哪份按请求体路由的文件；预写会话的两种不需要。 */
export function taskResultRouteFile(options: FixtureOptions): string | null {
  const scenario = options.taskResult;
  if (!scenario || scenario === 'missing' || scenario === 'bad-json') return null;
 // ：三份新场景各有自己的路由文件；其余沿用 的。
  if (
    scenario === 'say' ||
    scenario === 'resume' ||
    scenario === 'handback' ||
    scenario === 'seats'
  ) {
    return 't028-routes-' + scenario + '.json';
  }
 // 同轮两张卡、任务对话的 say 步。
  if (scenario === 'two' || scenario === 'pretext') {
    return 't031-routes-' + scenario + '.json';
  }
  return 't024-routes-' + scenario + '.json';
}


/** ：任务图场景的回放路由（Kind 建图/出卡，ThinAgent 报 ready）。 */
export function t052RouteFile(options: FixtureOptions): string | null {
  if (options.t052Fail) return 't052-routes-graph-fail.json';
  if (!options.t052Graph) return null;
  return 't052-routes-graph.json';
}


/** ：程序门场景的回放路由（Kind 提程序门图，ThinAgent 交被把关结果）。 */
export function t057RouteFile(options: FixtureOptions): string | null {
  if (options.t057 === undefined) return null;
  return 't057-routes-' + options.t057 + '.json';
}


/** ：账本场景的回放路由（Kind 带约束派任务，ThinAgent 记账本再报 ready）。 */
export function t056RouteFile(options: FixtureOptions): string | null {
  if (options.t056 !== 'ledger') return null;
  return 't056-routes-ledger.json';
}


/** ：里程碑门场景的回放路由（Kind 提图/出卡，ThinAgent 按 B→门→C 报 ready）。 */
export function t053RouteFile(options: FixtureOptions): string | null {
  if (!options.t053Gate) return null;
  return 't053-routes-graph.json';
}


/** ：工作页读侧的回放路由（board/history/confirm）。 */
export function t055RouteFile(options: FixtureOptions): string | null {
  if (!options.t055) return null;
  return 't055-routes-' + options.t055 + '.json';
}


/** ：调查任务（Kind 受理只读任务，ThinAgent 报 ready）。 */
export function t054RouteFile(options: FixtureOptions): string | null {
  if (!options.t054) return null;
  return 't054-routes-' + options.t054 + '.json';
}


/** ：写任务/接受/bash/工作树这些场景的回放路由。 */
export function t073RouteFile(options: FixtureOptions): string | null {
  if (!options.t073Cards) return null;
  return 't073-cards-routes.json';
}

export function t030RouteFile(options: FixtureOptions): string | null {
  if (!options.t030) return null;
  return 't030-routes-' + options.t030 + '.json';
}


/** 修复：写任务 ThinAgent 的合法生产回放（write/edit/bash + task_report）。 */
export function t013RouteFile(options: FixtureOptions): string | null {
  if (!options.t013ProcessTask) return null;
  return 't013-routes-process.json';
}
