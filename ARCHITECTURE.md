# AIOS 架构

- 本文件只写不常变的东西：代码地图、依赖方向、不变量、接缝。
- 产品与设计文档在 docs/（不提交）；这里不重复。

## 鸟瞰

AIOS 是一个本地 agent 内核：每次模型请求从会话日志派生，工具在本地执行，界面后续接 Tauri。
当前仓库只有第一个内核切片（T004 agent loop）的骨架。

## 代码地图

- crates/model：provider 无关的消息 IR（Message / ContentBlock / Usage / FinishReason / StreamChunk），DeepSeek 的 Messages 适配器（请求序列化、SSE 解码、流事件翻译、错误码）。
- crates/session：会话事件（turn/step/消息），只追加的 JSONL 存储（SessionStore），以及从日志派生请求消息。
- crates/tools：Tool 接口、ToolCall/ToolResult、ToolSet；内置工具在 T004.2 加入。
- crates/agent：一轮多步的驱动、收件箱（follow-up / steer / inject）、Agent 的 6 个驱动入口（create / resume / send / cancel / subscribe / idle）；另有 id / log_path 两个只读元数据访问器，不属于驱动入口。
- crates/cli：开发用 CLI `aios-dev`（`chat` / `run`）。

## 依赖方向

`cli → agent → tools → session → model`

- 下层不依赖上层。
- 跨层只走各 crate 的公开接口。

## 不变量

- **会话日志只追加**：`SessionStore::append` 是唯一追加事件的入口；`create` 写首行 header、`open` 只截掉未提交的尾行，不重写已提交内容。每批事件先写入再 fsync，写失败回滚到写之前的长度。
- **每次模型请求都从会话日志派生**：不保留一份会话内的独立消息副本。
- **公开接口上只有 `tools::Tool` 是 trait**：其他公开接缝用具体类型（`agent::Deps { model, tools, store }`）。`session` 的文件写入接缝 `LogWriter` 是内部接缝（生产用 `std::fs::File`，测试用它注入 fsync 失败），不暴露在公开接口上。测试替身放在 HTTP 层（wiremock）和临时目录上，不越过公开接口去 mock 内部类型。
- 一个会话一个 JSONL 文件，路径是 `<data-dir>/sessions/<id>.jsonl`；首行是 header（格式版本、id、cwd、创建时间）。

## 接缝

- 测试接缝 1：`Agent` 的 6 个入口（create / resume / send / cancel / subscribe / idle）。
- 测试接缝 2：内置工具的 `Tool::execute`（T004.2 起）。
- 模型 provider 在 HTTP 层替换（真 DeepSeek 适配器指向本地 mock）。
