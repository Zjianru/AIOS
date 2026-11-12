//! T010.1：不花 key 的模型回放（照 DSH test-support/llm-replay 的脚本推导与覆盖语义）。
//!
//! 输入是一份会话日志（JSONL）；脚本 = 日志里每条 assistant/message 和 assistant/attempt
//! 的 stream，按 seq 顺序一次调用一条。回放服务说 Messages 协议：第 N 次请求回第 N 份脚本，
//! 编码成 SSE。脚本用完再请求回 500 和一句说明。
//!
//! 覆盖文件（replay.override.json）：整份替换（ReplayEntry 数组），或 {patches:[{at, entry}]}
//! 换掉/追加第 N 条；entry 支持 stream/throw/cut/hang 四种。

mod encode;
mod script;
mod server;

pub use encode::{encode_sse, encode_sse_cut};
pub use script::{ReplayCall, ReplayError, ReplayRoute, ReplayScript};
pub use server::{ReplayServer, start_replay_server, start_replay_server_with_models};
