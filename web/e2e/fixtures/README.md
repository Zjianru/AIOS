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
| real.jsonl | T010.5 真 key 录的真跑会话（多步工具 + 思考 + 缓存读 + 手动压缩）；回放 pace 20 | 6（5 条 assistant/message + 1 条压缩摘要；含 2 段 reasoning、2 次工具调用） |
| usage.jsonl | T010.3 用量夹具：第一轮带 reasoning 帧（回放 pace 400）与缓存读写 | 8（首轮 usage 1000+200 读+50 写、输出 120） |
| failure-credential.json | text 的覆盖：第一次调用 401 | — |
| failure-model.json | text 的覆盖：连续 500 | 10 条 throw |
| failure-retry.json | text 的覆盖：3 条 500 后回 text 的正常流 | 3 throw + 1 stream |

## 真 key 夹具的来源（T010.5）

real.jsonl 用真 key 录制（key 只在运行时注入 Keyring，不进文件/命令行/证据）：

    DEEPSEEK_API_KEY=<runtime> AIOS_DOD_ROOT=/tmp/aios-t010-dod AIOS_DOD_KEEP=1       npx playwright test e2e/dod-t010.spec.ts
    # 跑完从 /tmp/aios-t010-dod/t010-real-fixture.jsonl 复制进本目录

录制内容：一次多步工具对话（read hello.txt，两轮工具调用）、一次追问、一次手动压缩、压缩后一轮；带思考帧与缓存读。录完先扫 key（前后 8 位，0 命中）再复制进本目录。

## 后续

T010.5 会用真 key 录的夹具（多步工具、思考、缓存读、手动压缩）替换这些；届时同步更新本 README。
