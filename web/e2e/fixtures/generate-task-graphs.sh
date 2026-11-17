#!/usr/bin/env bash
# 重生成任务图 e2e 夹具（三张已确认图 + 一个单派任务）。
# 任务 id 由 SQLite 随机生成，重跑后字节 hash 会变；页面只按标题/结构断言。
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p service --test it work_graph_fixture::write_task_graphs_fixture -- --ignored --exact --nocapture
node "$(dirname "$0")/normalize-task-graphs.mjs" "$(dirname "$0")/t062-task-graphs.sqlite"
shasum -a 256 web/e2e/fixtures/t062-task-graphs.sqlite
