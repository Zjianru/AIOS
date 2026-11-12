/**
 * 任务对话的场景标题：TaskChatScene 拉到任务事实后写进来，Toolbar 用它显示任务名字；
 * 切走任务对话（组件卸载或换任务）就清掉，别的场景不带着上一份标题。
 */
import { create } from 'zustand';

export const useTaskChromeStore = create<{
  taskTitle: string | null;
  setTaskTitle: (title: string | null) => void;
}>()((set) => ({
  taskTitle: null,
  setTaskTitle: (taskTitle) => set({ taskTitle }),
}));
