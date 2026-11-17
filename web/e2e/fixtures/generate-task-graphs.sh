#!/usr/bin/env bash
# 重生成任务图 e2e 夹具（三张已确认图 + 一个单派任务）。
# 生成后由 normalize-task-graphs.mjs 做确定性 id 重映射；同一构造每次重生成字节一致，脚本末尾打印 sha256。
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p service --test it work_graph_fixture::write_task_graphs_fixture -- --ignored --exact --nocapture
node "$(dirname "$0")/normalize-task-graphs.mjs" "$(dirname "$0")/t062-task-graphs.sqlite"
shasum -a 256 web/e2e/fixtures/t062-task-graphs.sqlite
