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

## T015.1 画布夹具（手写）

| 文件 | 场景 | 模型调用数 |
|---|---|---|
| canvas.jsonl | 画布：三个滑块 + 换标签的按钮，内容超过 420px | 2 |
| canvas-cdn.jsonl | 画布：清单里的 CDN 库；清单外的脚本/图片/fetch；/api、/ws、parent.document 探测 | 2 |
| canvas-loop.jsonl | 画布：脚本里转死循环 | 2 |
| canvas-versions.jsonl | 画布：同一画布两版（第 2 版用 edits），再一条第三轮回答（T015.2） | 3 |
| canvas-send.jsonl | 画布：「按这个存」按钮（aios.send），第 2 轮回答另备一条（T015.4） | 2 |
| canvas-streaming.jsonl | 画布：canvas 参数拆成 4 段慢慢流（回放 pace 120ms），看「正在画 · 标题」（T015.5） | 2 |
| canvas-errors.jsonl | 画布：脚本报错 / 未处理 rejection / CSP 违规三个按钮（T015.5） | 2 |

这些是手写的，只放模型回放要的 `assistant/message`：第 1 条回一次 `canvas` 工具调用（HTML 在参数里），第 2 条回一句回答（versions 多一条第三轮）。**工具调用由真内核执行**，所以页面显示的画布一定来自日志的 `tool/result.meta.canvas`，不是夹具里写死的结果。`canvas-streaming` 只把 `tool-call-chunks` 的 args 拆成几段；完整参数（`message.content` 和 block-end）不变。

## 真 key 夹具的来源（T010.5）

real.jsonl 用真 key 录制（key 只在运行时注入 Keyring，不进文件/命令行/证据）：

    DEEPSEEK_API_KEY=<runtime> AIOS_DOD_ROOT=/tmp/aios-t010-dod AIOS_DOD_KEEP=1       npx playwright test e2e/dod-t010.spec.ts
    # 跑完从 /tmp/aios-t010-dod/t010-real-fixture.jsonl 复制进本目录

录制内容：一次多步工具对话（read hello.txt，两轮工具调用）、一次追问、一次手动压缩、压缩后一轮；带思考帧与缓存读。录完先扫 key（前后 8 位，0 命中）再复制进本目录。

## 后续

T010.5 会用真 key 录的夹具（多步工具、思考、缓存读、手动压缩）替换这些；届时同步更新本 README。
