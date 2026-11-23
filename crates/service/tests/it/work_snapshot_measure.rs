//! 200 任务的 Work 快照测量入口（手工跑，不进常规门禁）。
//!
//! 构造：20 张已确认图 × 10 个链式依赖的子任务 = 200 个任务；确认后把 launch 效果排空，
//! 让快照测的是稳态读面（没有排队起跑的任务）。
//! 命令：cargo test -p service --test it work_snapshot_measure -- --ignored --nocapture

use std::time::Instant;

use tasks::{
    ConversationId, EffectKind, ModelChoice, NewGraph, NewSubtask, NewTask, Store, TaskPolicy,
};
use tempfile::tempdir;

fn draft(cwd: &std::path::Path, title: &str) -> NewTask {
    NewTask {
        project_id: None,
        cwd_source: None,
        title: title.to_owned(),
        requirement: format!("{title}：测量用任务"),
        dod: format!("{title} 的 DoD"),
        cwd: cwd.to_path_buf(),
        conversation: ConversationId::new("kind-work-measure"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

#[tokio::test]
#[ignore = "200 任务 Work 快照测量；手工跑，不进常规门禁"]
async fn measure_work_snapshot_on_200_tasks() {
    const GRAPHS: usize = 20;
    const PER_GRAPH: usize = 10;
    const ITERATIONS: usize = 10;
    let t0 = 1_790_000_000_000_i64;
    let directory = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    for graph in 0..GRAPHS {
        let mut subtasks = Vec::with_capacity(PER_GRAPH);
        for index in 0..PER_GRAPH {
            subtasks.push(NewSubtask {
                task: draft(cwd.path(), &format!("测量图 {graph} 任务 {index}")),
                policy: TaskPolicy::default(),
                after: if index == 0 {
                    Vec::new()
                } else {
                    vec![index - 1]
                },
                milestone: false,
                program_gate: Vec::new(),
            });
        }
        let root = store
            .create_graph(
                NewGraph {
                    root: draft(cwd.path(), &format!("测量图 {graph}")),
                    subtasks,
                },
                t0 + i64::try_from(graph).unwrap(),
            )
            .unwrap();
        store
            .confirm_graph(&root.id, t0 + 100 + i64::try_from(graph).unwrap())
            .unwrap();
    }
    // 排空确认图排下的 launch 效果：测量稳态读面。
    while let Some(claim) = store.claim_effect(EffectKind::Launch, t0 + 200).unwrap() {
        store.complete_effect(&claim).unwrap();
    }
    drop(store);

    let host = crate::common::open(crate::common::options(directory.path())).await;
    // 预热一次，避免把首次读库/建表算进。
    let _ = host
        .dispatch(service::Call::Work(service::WorkRequest {}))
        .await
        .expect("warmup work");

    let mut total = Vec::with_capacity(ITERATIONS);
    let mut encode = Vec::with_capacity(ITERATIONS);
    let mut response = None;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let next = host
            .dispatch(service::Call::Work(service::WorkRequest {}))
            .await
            .expect("work");
        total.push(start.elapsed().as_secs_f64() * 1000.0);
        let service::Response::Work(work) = next else {
            panic!("work 响应");
        };
        let start = Instant::now();
        let bytes = serde_json::to_vec(&work).expect("serialize work");
        encode.push(start.elapsed().as_secs_f64() * 1000.0);
        if response.is_none() {
            response = Some((
                work.tasks.len(),
                work.graphs.len(),
                work.graphs
                    .iter()
                    .map(|graph| graph.edges.len())
                    .sum::<usize>(),
                bytes.len(),
            ));
        }
    }
    let (tasks_in_response, graphs, edges, json_bytes) = response.expect("response");
    let mut sorted = total.clone();
    sorted.sort_by(f64::total_cmp);
    let mut encode_sorted = encode.clone();
    encode_sorted.sort_by(f64::total_cmp);
    println!(
        "WORK_MEASURE tasks_created={} graphs_created={} iterations={} tasks_in_response={tasks_in_response} graphs_in_response={graphs} edges={edges} json_bytes={json_bytes} total_ms_min={:.3} total_ms_median={:.3} total_ms_max={:.3} encode_ms_min={:.3} encode_ms_median={:.3} encode_ms_max={:.3} os={} arch={} parallelism={:?}",
        200,
        GRAPHS,
        ITERATIONS,
        sorted[0],
        sorted[ITERATIONS / 2],
        sorted[ITERATIONS - 1],
        encode_sorted[0],
        encode_sorted[ITERATIONS / 2],
        encode_sorted[ITERATIONS - 1],
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .ok(),
    );
}
