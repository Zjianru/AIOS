//! 流式文字 store 与可用模型缓存。

import { create } from 'zustand';
import type { AvailableModelRow } from '@/data/service';

/** ：连接 id → 模型目录（第一次用到时拉一次；价格/窗口/思考档从这里来）。 */
export const catalogCache = new Map<string, AvailableModelRow[]>();

/** 流式文本单独放一个 store：只有最后一条在重绘，别的消息不跟着重建。 */
export const useStreamStore = create<{ turnId: string | null; itemId: string | null; text: string }>(() => ({
  turnId: null,
  itemId: null,
  text: '',
}));
