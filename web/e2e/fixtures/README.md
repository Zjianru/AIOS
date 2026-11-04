# e2e 回放夹具（T010.2）

这些是会话日志（JSONL），给 `web/e2e/service-fixture.ts` 起的 `aios-replay` 用；不是产品数据，不含 key。

## 怎么来的

T010.2 时用一次性 Playwright 录制器（已删除，不在仓库）跑改造前的手写 SSE 假模型：起 serve，按场景发消息/答问/压缩，跑完把 `<dataDir>/sessions/*.jsonl` 复制到这里。覆盖文件（`failure-*.json`）由脚本从 `text.jsonl` 的 stream 与错误 entry 生成。

## 场景与每次调用数

| 文件 | 场景 | 模型调用数 |
|---|---|---|
| text.jsonl | 默认单轮文本 | 8 |
| tool.jsonl | 工具轮（read hello.txt 后回答） | 2 |
| ask.jsonl | ask_user_question 两题作答后回答 | 2 |
| high-usage.jsonl | 高上下文（4500/5000）自动压缩 + 手动压缩 + 再一轮 | 5（2 条 assistant/message + 3 条 compaction/summary 的 llmStreamCall） |
| usage.jsonl | T010.3 用量夹具：第一轮带 reasoning 帧（回放 pace 400）与缓存读写 | 8（首轮 usage 1000+200 读+50 写、输出 120） |
| failure-credential.json | text 的覆盖：第一次调用 401 | — |
| failure-model.json | text 的覆盖：连续 500 | 10 条 throw |
| failure-retry.json | text 的覆盖：3 条 500 后回 text 的正常流 | 3 throw + 1 stream |

## 后续

T010.5 会用真 key 录的夹具（多步工具、思考、缓存读、手动压缩）替换这些；届时同步更新本 README。
