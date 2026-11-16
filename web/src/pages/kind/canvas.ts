//! 画布：画布版本类型、从日志折叠版本与合并 live 版本。

import type { ServiceEvent } from '@/data/service';

/**
 * 画布：日志里 tool/result.meta.canvas 的那一版全文。
 * 回放和实时都走同一份事件，所以这里只有日志事实，没有页面自己的状态。
 */
export interface CanvasViewDoc {
  id: string;
 /** 第几版（ 只出第 1 版；版本切换在）。 */
  n: number;
  title: string;
  note: string;
  html: string;
}


/** 画布的一个版本（日志里的一条 tool/result.meta.canvas）。 */
export interface CanvasVersionView {
 /** 第几版，从 1 开始。 */
  n: number;
  title: string;
  note: string;
  html: string;
}


/** 一个画布出过的版本（按 n 升序）和最新版在哪一轮。 */
export interface CanvasTrack {
  latest: number;
 /** 最新一版所在轮次的视图 id（旧版缩成一行时点它跳过去）。 */
  latestTurnId: string;
  versions: CanvasVersionView[];
}


/** 从日志折叠每个画布的版本：只认 tool/result.meta.canvas。 */
export function canvasTracks(events: ServiceEvent[]): Record<string, CanvasTrack> {
  return mergeCanvasTracks({}, events);
}


/** 把一批事件折进已有的版本表（首屏、往上翻、live 三处共用）。 */
export function mergeCanvasTracks(
  tracks: Record<string, CanvasTrack>,
  events: ServiceEvent[],
): Record<string, CanvasTrack> {
  let next = tracks;
  for (const event of events) {
    if (event.type !== 'tool/result') continue;
    const turn = typeof event.data.turn === 'number' ? event.data.turn : null;
    if (turn === null) continue;
    const version = canvasVersionOf(event.data.meta);
    if (!version) continue;
    const track = next[version.id] ?? { latest: 0, latestTurnId: '', versions: [] };
    const versions = track.versions
      .filter((item) => item.n !== version.n)
      .concat({ n: version.n, title: version.title, note: version.note, html: version.html })
      .sort((left, right) => left.n - right.n);
    next = {
      ...next,
      [version.id]: {
        latest: Math.max(track.latest, version.n),
 // 同一版重复到达时认后一次（live 重放同一条事件不该丢轮次）。
        latestTurnId: version.n >= track.latest ? 'turn-' + String(turn) : track.latestTurnId,
        versions,
      },
    };
  }
  return next;
}


/** ：user/message 文字块上的 canvas（{id, title} 都是字符串才算；形状不对就当普通文字）。 */
export function canvasSourceOf(value: unknown): { id: string; title: string } | null {
  if (!value || typeof value !== 'object') return null;
  const typed = value as { id?: unknown; title?: unknown };
  if (typeof typed.id !== 'string' || typeof typed.title !== 'string') return null;
  return { id: typed.id, title: typed.title };
}


/** tool/result.meta.canvas → 一版画布（形状不对就当没有）。 */
export function canvasVersionOf(meta: unknown): (CanvasVersionView & { id: string }) | null {
  if (!meta || typeof meta !== 'object') return null;
  const value = (meta as { canvas?: unknown }).canvas;
  if (!value || typeof value !== 'object') return null;
  const typed = value as { id?: unknown; n?: unknown; title?: unknown; note?: unknown; html?: unknown };
  if (typeof typed.id !== 'string' || typeof typed.title !== 'string' || typeof typed.html !== 'string') {
    return null;
  }
  return {
    id: typed.id,
    n: typeof typed.n === 'number' ? typed.n : 1,
    title: typed.title,
    note: typeof typed.note === 'string' ? typed.note : '',
    html: typed.html,
  };
}
