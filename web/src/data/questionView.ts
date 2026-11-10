/**
 * Kind 页看到的提问形状（fixtures 和轮次视图共用）。
 *
 * 线上 question/asked 的 data 是事件自己的 JSON（session::EventKind::QuestionAsked），
 * 服务没有对应的具名 wire 类型；这两个是页面自己的视图形状，不属于生成协议。
 */

export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  id: string;
  header?: string;
  question: string;
  options?: AskOption[];
  multiSelect?: boolean;
}
