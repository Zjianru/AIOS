//! 写任务与门：程序门、任务图、工作目录、结果卡与里程碑。

use super::*;

/// 写任务：写入范围落库、工具名单正好 15 件（+write/edit/office）；task_accept 的 schema
/// 有 write_scope/worktree，没有 bash 字段（bash 开关归另一张票）。
#[tokio::test]
async fn a_write_task_freezes_the_scope_and_eleven_tools() {
    let fixture = Fixture::new().await;
    let target = fixture.cwd.path().join("target.py");
    std::fs::write(&target, "old\n").unwrap();
    let task = fixture.accept_write(true, "target.py").await;

    let scope = std::fs::canonicalize(&target).unwrap();
    assert_eq!(task.write_scope.as_deref(), Some(scope.as_path()));
    assert_eq!(task.requirement_version(), 1);
    assert_eq!(
        task.versions[0].write_scope.as_deref(),
        Some(scope.as_path()),
        "第一条要求版本带着写入范围"
    );

    let mut tools = task.tools.clone();
    tools.sort();
    let mut expected = vec![
        "edit",
        "glob",
        "grep",
        "office_create",
        "office_read",
        "read",
        "skill",
        "task_handback",
        "task_ledger_history",
        "task_ledger_note",
        "task_report",
        "todo_write",
        "web_fetch",
        "web_search",
        "write",
    ];
    expected.sort();
    assert_eq!(tools, expected, "写任务冻结 15 件工具");

    let kind_body = fixture.router.bodies(false).remove(0);
    let request: Value = serde_json::from_str(&kind_body).unwrap();
    let accept = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "task_accept")
        .expect("task_accept 的工具定义");
    let properties = &accept["input_schema"]["properties"];
    assert!(properties.get("write_scope").is_some(), "{properties}");
    assert!(properties.get("worktree").is_some(), "{properties}");
    assert!(
        properties.get("bash").is_none(),
        "bash 不能成为 task_accept 字段：{properties}"
    );
}

/// Kind 的工具面：能读、能派/问/停/改任务，没有写、改、跑命令和三件后台任务。
#[tokio::test]
async fn kind_has_no_write_shell_or_background_tools() {
    let fixture = Fixture::new().await;
    fixture.router.push_kind(crate::common::text_sse("好"));
    fixture
        .host
        .dispatch(Call::Send(send(
            &fixture.host,
            "kind-1",
            "看一下你能做什么",
        )))
        .await
        .expect("发送");
    crate::common::wait_turns(&fixture.host, fixture.data(), 1).await;

    let body = fixture.router.bodies(false).remove(0);
    let names = tool_names(&body);
    for banned in [
        "write",
        "edit",
        "apply_patch",
        "bash",
        "job_output",
        "job_list",
        "job_kill",
    ] {
        assert!(
            !names.iter().any(|name| name == banned),
            "Kind 不该有 {banned}：{names:?}"
        );
    }
    for required in [
        "read",
        "glob",
        "grep",
        "skill",
        "task_accept",
        "task_status",
        "task_stop",
        "task_update",
        "ask_user_question",
        "canvas",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "Kind 要有 {required}：{names:?}"
        );
    }
}

/// 写入范围外的写被拒：错误码和原文固定、文件内容与 mtime 都没变，之后能交回 Kind。
#[tokio::test]
async fn an_out_of_scope_write_is_refused_unchanged_and_hands_back() {
    let fixture = Fixture::new().await;
    let target = fixture.cwd.path().join("target.py");
    let outside = fixture.cwd.path().join("outside.py");
    std::fs::write(&target, "inside\n").unwrap();
    std::fs::write(&outside, "outside\n").unwrap();
    let outside_mtime = std::fs::metadata(&outside).unwrap().modified().unwrap();

    fixture.router.push_thin(crate::common::tool_use_sse(
        1,
        "call-write",
        "write",
        &serde_json::json!({"file_path": "outside.py", "content": "changed\n"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::tool_use_sse(
        2,
        "call-handback",
        "task_handback",
        &serde_json::json!({"reason": "用户要改写入范围外的文件"}).to_string(),
    ));
    fixture.router.push_thin(crate::common::text_sse("交回"));
    let task = fixture.accept_write(false, "target.py").await;
    wait_until("交回 relay", || {
        !relay_messages(&crate::common::read_root_events(fixture.data())).is_empty()
    })
    .await;

    let store = tasks_store(fixture.data());
    let task_row = store.task(&task.id).unwrap().unwrap();
    let attempt = task_row.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let (text, error) = tool_result(&events, "call-write");
    let scope = std::fs::canonicalize(&target).unwrap();
    assert_eq!(error, Some(true), "{text}");
    assert_eq!(
        text,
        format!(
            "Error: 这个路径不在写入范围里（写入范围：{}）。要改范围外的，调 task_handback。",
            scope.display()
        ),
        "拒绝原文固定"
    );
    let code = events.iter().find_map(|event| match &event.kind {
        session::EventKind::ToolResult { message, error, .. }
            if message.tool_call_id.as_deref() == Some("call-write") =>
        {
            error.as_ref().map(|error| error.code.clone())
        }
        _ => None,
    });
    assert_eq!(code.as_deref(), Some("WRITE_OUT_OF_SCOPE"), "拒绝码固定");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "outside\n",
        "越界写不能改文件内容"
    );
    assert_eq!(
        std::fs::metadata(&outside).unwrap().modified().unwrap(),
        outside_mtime,
        "越界写连 mtime 都不能动"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "inside\n");

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    assert_eq!(relays.len(), 1, "{relays:?}");
    assert_eq!(relays[0].1["state"], "blocked");
    assert_eq!(relays[0].1["origin"], "agent");
    assert_eq!(relays[0].1["summary"], "用户要改写入范围外的文件");
    assert_eq!(relays[0].1["question"], "这超出了任务范围，要不要改任务？");
    assert_eq!(
        attempt.end,
        Some(AttemptEnd::Reported),
        "交回结束这个 Attempt"
    );
}

/// bash 关着：写任务还是 15 件、没有 bash 和后台任务，结果卡数据里也没有 bash 说明。
#[tokio::test]
async fn a_write_task_without_bash_gets_eleven_tools_and_no_bash_note() {
    let fixture = Fixture::new().await;
    save_exa_key(fixture.data());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-skill",
            "skill",
            &serde_json::json!({"name": "web"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(true, write_task_args(), 1).await;
    assert!(!task.bash, "默认关：写任务不冻结 bash");
    assert_eq!(
        sorted(task.tools.clone()),
        sorted(WRITE_TOOLS),
        "写任务 15 件"
    );

    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let body = &relays[0].1;
    assert_eq!(body["state"], "ready", "{body}");
    assert!(
        body.get("bash_note").is_none(),
        "关着 bash 不写说明：{body}"
    );

    // 插件默认关：冻结名单里有 office_*，实际请求工具表按开关过滤掉。
    let names = sorted(
        tool_names(&loaded_request(&fixture.router))
            .into_iter()
            .filter(|name| !name.starts_with("office_"))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        names,
        sorted(
            WRITE_TOOLS
                .iter()
                .copied()
                .filter(|name| !name.starts_with("office_"))
        ),
        "ThinAgent 的工具表 13 件（插件关）"
    );

    let bodies = fixture.router.bodies(true);
    assert!(
        bodies[0].contains("You cannot run shell commands"),
        "关着 bash 的角色规则说明没有跑命令的手段"
    );
    assert!(
        !bodies[0].contains("You also have bash and the background job tools"),
        "关着 bash 不给跑命令的说法"
    );
}

/// 开关打开：受理时冻结 15 件；之后关掉开关不影响已派的任务（起跑仍用 15 件）。
#[tokio::test]
async fn the_bash_switch_on_freezes_fifteen_tools_into_the_write_task() {
    let fixture = Fixture::new().await;
    save_exa_key(fixture.data());
    set_bash(&fixture.host, true).await;
    assert!(
        service::settings::load(fixture.data()).exec.bash,
        "开关写进设置文件"
    );

    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-skill",
            "skill",
            &serde_json::json!({"name": "web"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(true, write_task_args(), 1).await;
    assert!(task.bash, "创建时冻结打开");
    assert_eq!(
        sorted(task.tools.clone()),
        sorted(WRITE_BASH_TOOLS),
        "开了 bash 的写任务冻结 19 件"
    );
    for name in BASH_JOB_TOOLS {
        assert!(
            task.tools.iter().any(|tool| tool == name),
            "冻结名单要有 {name}：{:?}",
            task.tools
        );
    }

    // 创建之后关掉开关：任务行不跟着变，起跑也用冻结的 15 件。
    set_bash(&fixture.host, false).await;
    assert!(!service::settings::load(fixture.data()).exec.bash);
    let frozen = tasks_store(fixture.data()).task(&task.id).unwrap().unwrap();
    assert!(frozen.bash, "已派任务的 bash 不跟着开关变");
    assert_eq!(sorted(frozen.tools.clone()), sorted(WRITE_BASH_TOOLS));

    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;

    let names = sorted(
        tool_names(&loaded_request(&fixture.router))
            .into_iter()
            .filter(|name| !name.starts_with("office_"))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        names,
        sorted(
            WRITE_BASH_TOOLS
                .iter()
                .copied()
                .filter(|name| !name.starts_with("office_"))
        ),
        "ThinAgent 的工具表 17 件（插件关）"
    );

    let bodies = fixture.router.bodies(true);
    assert!(
        bodies[0].contains("You also have bash and the background job tools"),
        "开着 bash 的角色规则说清楚能跑命令"
    );
    assert!(
        !bodies[0].contains("You cannot run shell commands"),
        "开着 bash 不能再说不许跑命令"
    );
}

/// 开了 bash 的写任务，结果卡数据带固定说明（原文来自 spec 故事 7）。
#[tokio::test]
async fn a_bash_task_card_carries_the_fixed_note() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(1, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));

    let task = fixture.accept_args(false, write_task_args(), 1).await;
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let body = &relays[0].1;
    assert_eq!(body["task"]["id"], task.id.as_str(), "{body}");
    assert_eq!(
        body["bash_note"], "这个任务开了 bash，bash 的改动不在清单里",
        "说明原文进卡数据"
    );
}

/// 只读任务：开关打开也不加 bash 和后台任务，还是 9 件。
#[tokio::test]
async fn a_read_only_task_never_gets_bash_even_when_the_switch_is_on() {
    let fixture = Fixture::new().await;
    save_exa_key(fixture.data());
    set_bash(&fixture.host, true).await;
    assert!(service::settings::load(fixture.data()).exec.bash);

    fixture.router.push_thin_for_title(
        "查一下",
        crate::common::tool_use_sse(
            1,
            "call-skill",
            "skill",
            &serde_json::json!({"name": "web"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "查一下",
        report_sse(2, "call-report", "ready", "查好了", None),
    );
    fixture
        .router
        .push_thin_for_title("查一下", crate::common::text_sse("done"));

    let task = fixture
        .accept_args(
            true,
            serde_json::json!({
                "title": "查一下",
                "requirement": "找答案",
                "dod": "有答案"
            }),
            1,
        )
        .await;
    assert!(!task.bash, "只读任务不冻结 bash");
    assert_eq!(
        sorted(task.tools.clone()),
        sorted(READ_ONLY_TOOLS),
        "只读任务还是 12 件"
    );

    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    let names = sorted(
        tool_names(&loaded_request(&fixture.router))
            .into_iter()
            .filter(|name| !name.starts_with("office_"))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        names,
        sorted(
            READ_ONLY_TOOLS
                .iter()
                .copied()
                .filter(|name| !name.starts_with("office_"))
        ),
        "只读任务给 ThinAgent 的工具表 11 件（插件关）"
    );
    for name in BASH_JOB_TOOLS {
        assert!(
            !names.iter().any(|tool| tool == name),
            "只读任务不给 {name}：{names:?}"
        );
    }
}

/// worktree:true：第一次 Attempt 起跑前在 cwd 所在的仓库开工作树，ThinAgent 的 cwd
/// 换到工作树里对应的 sub；写落到工作树，原目录不动；卡数据带工作树路径。
#[tokio::test]
async fn a_worktree_task_runs_in_the_worktree_and_writes_only_there() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            1,
            "call-read",
            "read",
            &serde_json::json!({"file_path": "app.py"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        crate::common::tool_use_sse(
            2,
            "call-write",
            "write",
            &serde_json::json!({"file_path": "app.py", "content": "changed\n"}).to_string(),
        ),
    );
    fixture.router.push_thin_for_title(
        "改一下判重",
        report_sse(2, "call-report", "ready", "改好了", None),
    );
    fixture
        .router
        .push_thin_for_title("改一下判重", crate::common::text_sse("done"));
    let task = fixture.accept_args(false, worktree_task_args(), 1).await;
    let store = tasks_store(fixture.data());
    wait_until("worktree ready", || reported_ready(&store, &task.id)).await;
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;

    let worktree_root = worktree_root(&fixture, &task);
    let worktree_cwd = worktree_cwd(&fixture, &task);
    assert_eq!(
        worktree_cwd.strip_prefix(&worktree_root).unwrap(),
        Path::new("sub"),
        "工作树里的子目录对应原 cwd 的相对位置"
    );

    let stored = store.task(&task.id).unwrap().unwrap();
    let attempt = stored.latest_attempt().expect("Attempt").clone();
    let events = thin_events(fixture.data(), &attempt);
    let assignment = assignment(&events);
    assert_eq!(
        assignment["cwd"],
        serde_json::json!(worktree_cwd.display().to_string()),
        "assignment 的 cwd 在工作树里"
    );
    assert_eq!(
        assignment["write_scope"],
        serde_json::json!(worktree_cwd.join("app.py").display().to_string()),
        "写入范围按同样的相对位置换算"
    );
    let bodies = fixture.router.bodies(true);
    assert!(
        bodies
            .iter()
            .any(|body| request_working_directory(body) == worktree_cwd.display().to_string()),
        "ThinAgent 请求的工作目录在工作树里：{bodies:?}"
    );

    assert_eq!(
        std::fs::read_to_string(fixture.cwd.path().join("sub").join("app.py")).unwrap(),
        "old\n",
        "原目录文件不改"
    );
    assert_eq!(
        std::fs::read_to_string(worktree_cwd.join("app.py")).unwrap(),
        "changed\n",
        "写落在工作树里"
    );

    let repo = fixture.cwd.path();
    let listed = listed_worktrees(repo);
    assert_eq!(listed.len(), 2, "原仓库加一个工作树：{listed:?}");
    assert!(
        listed.contains(&worktree_root),
        "工作树登记在这个仓库里：{listed:?}"
    );
    let branches = git(repo, &["branch", "--list", &format!("aios/{}", task.id)]);
    assert_eq!(
        branches.matches(&format!("aios/{}", task.id)).count(),
        1,
        "工作树分支只有一条：{branches}"
    );

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let (_, ready) = relays
        .iter()
        .find(|(_, body)| body["state"] == "ready")
        .expect("ready relay");
    assert_eq!(ready["task"]["stage"], "ready_for_review");
    assert_eq!(
        ready["worktree_path"],
        serde_json::json!(worktree_root.display().to_string()),
        "卡数据带工作树路径"
    );
    assert_eq!(ready["card_state"], "review", "{ready}");
    assert_eq!(
        ready["changed"],
        serde_json::json!(["app.py"]),
        "清单相对工作树里的 cwd：{ready}"
    );
    assert_eq!(
        ready["changed_line"], "改了 1 个文件（写入范围 app.py）",
        "{ready}"
    );
}

/// cwd 不是 git 仓库：不开会话，进待补足条件，卡上写固定的说明。
#[tokio::test]
async fn a_worktree_task_outside_a_repository_waits_for_conditions() {
    let fixture = Fixture::new().await;
    let mut args = worktree_task_args();
    args["cwd"] = serde_json::json!(".");
    let task = fixture.accept_args(false, args, 1).await;
    let store = tasks_store(fixture.data());
    wait_until("非仓库卡", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "blocked")
    })
    .await;

    let stored = store.task(&task.id).unwrap().unwrap();
    assert!(
        stored
            .missing
            .iter()
            .any(|item| item.as_str() == "这个目录不是 git 仓库，开不了工作树"),
        "缺项原文固定：{:?}",
        stored.missing
    );
    let attempt = stored.latest_attempt().expect("缺项 Attempt");
    assert_eq!(attempt.end, Some(AttemptEnd::NotOpened));
    assert_eq!(attempt.conversation, None, "非仓库不开会话");
    assert!(
        !fixture
            .data()
            .join("worktrees")
            .join(task.id.as_str())
            .exists(),
        "没开出工作树"
    );
    assert!(
        fixture.router.bodies(true).is_empty(),
        "非仓库不该给 ThinAgent 发请求"
    );
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::NeedsConditions,
        "进待补足条件"
    );

    let relays = relay_messages(&crate::common::read_root_events(fixture.data()));
    let (_, card) = relays
        .iter()
        .find(|(_, body)| body["state"] == "blocked")
        .expect("blocked 卡");
    assert_eq!(card["origin"], "package");
    assert_eq!(card["task"]["stage"], "needs_conditions");
    assert_eq!(
        card["summary"], "任务包缺：这个目录不是 git 仓库，开不了工作树",
        "卡上写固定说明"
    );
}

/// 工作树任务的写入范围在原仓库外面：换算不进工作树，按缺项停机，不悄悄写回原目录。
#[tokio::test]
async fn a_worktree_task_with_an_outside_scope_waits_for_conditions() {
    let fixture = Fixture::new().await;
    init_git_repo(fixture.cwd.path());
    let outside = fixture.data().join("outside.txt");
    let mut args = worktree_task_args();
    args["write_scope"] = serde_json::json!(outside.to_string_lossy());
    let task = fixture.accept_args(false, args, 1).await;
    let store = tasks_store(fixture.data());
    wait_until("范围换算不了卡", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "blocked")
    })
    .await;

    let stored = store.task(&task.id).unwrap().unwrap();
    assert!(
        stored
            .missing
            .iter()
            .any(|item| item.as_str().contains("工作树换算会落到工作树外面")),
        "缺项说明范围换算不了：{:?}",
        stored.missing
    );
    assert_eq!(
        stored
            .latest_attempt()
            .and_then(|attempt| attempt.conversation.clone()),
        None,
        "换算不了就不开会话"
    );
    assert!(
        !fixture
            .data()
            .join("worktrees")
            .join(task.id.as_str())
            .exists(),
        "没开出工作树"
    );
    assert!(
        fixture.router.bodies(true).is_empty(),
        "不该给 ThinAgent 发请求"
    );
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
}

/// 调查任务不变：ready 的卡上没有写任务的卡状态和清单字段。
#[tokio::test]
async fn an_investigation_ready_card_has_no_write_card_data() {
    let fixture = Fixture::new().await;
    fixture
        .router
        .push_thin(report_sse(1, "call-report", "ready", "查好了", None));
    fixture.router.push_thin(crate::common::text_sse("done"));
    let _task = fixture.accept(true).await;
    fixture
        .host
        .task_runtime()
        .expect("runtime")
        .set_pump_paused(false);

    let (_, body) = ready_relay(&fixture).await;
    assert_eq!(body["state"], "ready", "{body}");
    assert_eq!(body["task"]["stage"], "ready_for_review");
    for key in [
        "write_scope",
        "changed",
        "changed_line",
        "card_state",
        "bash_note",
        "worktree_path",
    ] {
        assert!(body.get(key).is_none(), "调查任务卡上没有 {key}：{body}");
    }
}

/// 开了 bash 的写任务：卡上有固定说明，也有改过的文件清单和 N/范围一行。
#[tokio::test]
async fn a_bash_write_task_card_notes_bash_and_lists_the_changes() {
    let fixture = Fixture::new().await;
    set_bash(&fixture.host, true).await;
    let task = run_write_task(
        &fixture,
        scoped_write_args(),
        vec![
            write_file_sse("call-w1", "a.py", "one\n"),
            report_sse(2, "call-report", "ready", "改好了", None),
            crate::common::text_sse("done"),
        ],
    )
    .await;
    assert!(task.bash, "受理时冻结了 bash");

    let (_, body) = ready_relay(&fixture).await;
    assert_eq!(
        body["bash_note"], "这个任务开了 bash，bash 的改动不在清单里",
        "{body}"
    );
    assert_eq!(body["changed"], serde_json::json!(["a.py"]), "{body}");
    assert_eq!(
        body["changed_line"], "改了 1 个文件（写入范围 .）",
        "{body}"
    );
    assert_eq!(body["card_state"], "review", "{body}");
}

// ===== ：任务图：提议 → 确认卡 → 确认 → 第一项开跑 =====

/// 提议两项 A→B → 出确认卡（两行事实）→ 确认 → 只放 A；再确认不重复执行。
#[tokio::test]
async fn graph_propose_card_confirm_launches_first_subtask_only() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);

    host.dispatch(Call::Send(send(
        &host,
        "kind-1",
        "先查 notes.md 有几行，再把行数写进 other.txt",
    )))
    .await
    .expect("发送");
    wait_until("建图并出卡", || {
        let store = tasks_store(data.path());
        let ids = store.task_ids().unwrap_or_default();
        ids.len() == 3
            && crate::common::read_root_events(data.path())
                .iter()
                .any(|event| matches!(&event.kind, session::EventKind::ToolResult { message, meta: Some(_), .. } if message.tool_call_id.as_deref() == Some("call-graph-card")))
    })
    .await;

    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    assert_eq!(children.len(), 2);
    assert_eq!(children[1].needs.len(), 1);
    assert_eq!(children[1].needs[0].from, children[0].id);

    // Kind 工具表里有 task_graph_propose。
    let kind_body = router.bodies(false).into_iter().next().expect("Kind 请求");
    assert!(
        tool_names(&kind_body)
            .iter()
            .any(|name| name == "task_graph_propose"),
        "Kind 工具表：{:?}",
        tool_names(&kind_body)
    );

    // 确认卡：一个选项、每个子任务一行事实（标题/等谁/写入范围）。
    let card = graph_card(data.path());
    let options = card["options"].as_array().expect("options");
    assert_eq!(options.len(), 1);
    let facts = options[0]["facts"].as_array().expect("facts");
    assert_eq!(facts.len(), 2, "每项一行事实：{facts:?}");
    assert!(
        facts[1]["name"]
            .as_str()
            .unwrap_or_default()
            .contains("写结果"),
        "第二行是下游子任务：{facts:?}"
    );
    assert!(
        facts[0]["value"]
            .as_str()
            .unwrap_or_default()
            .contains("等：无"),
        "第一项不等人：{facts:?}"
    );
    assert!(
        facts[1]["value"]
            .as_str()
            .unwrap_or_default()
            .contains("等：先查行数"),
        "第二项写明等谁：{facts:?}"
    );
    assert!(
        facts[1]["value"]
            .as_str()
            .unwrap_or_default()
            .contains("写入范围：只读"),
        "第二项写明写入范围：{facts:?}"
    );

    let confirm = Call::CardAct(CardActRequest {
        call_id: "call-graph-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),

        note: None,
        report_id: None,
    });
    let Response::CardAct(response) = host.dispatch(confirm.clone()).await.expect("确认") else {
        panic!("card-act 响应");
    };
    assert!(
        response
            .decision
            .as_ref()
            .is_some_and(|decision| decision.phase == session::CardDecisionPhase::Confirmed),
        "确认成功：{response:?}"
    );

    // 只放没有 needs 的第一项。
    let claim = store
        .claim_effect(EffectKind::Launch, i64::MAX / 2)
        .expect("取效果")
        .expect("A 的 launch");
    let tasks::EffectPayload::Launch { task_id } = claim.payload.clone() else {
        panic!("launch 载荷");
    };
    assert_eq!(task_id, children[0].id);
    store.complete_effect(&claim).expect("完成 launch");
    assert!(
        store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none(),
        "B 不能启动"
    );
    assert_eq!(
        store.stage(&children[1].id).expect("阶段"),
        Stage::NeedsConditions
    );
    assert_eq!(
        store.stage(&root.id).expect("阶段"),
        Stage::NeedsConditions,
        "总任务汇总取最靠前的子任务阶段"
    );

    // 重复确认：原样返回已记的决定，不再执行。
    let Response::CardAct(again) = host.dispatch(confirm).await.expect("再确认") else {
        panic!("card-act 响应");
    };
    assert!(again.decision.is_some());
    assert!(
        store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none(),
        "重复确认不重复启动"
    );

    // 卡片出了以后 Kind 这一轮就结束了；再发一句，下一轮动态脚本读 task_status 的总任务汇总。
    host.dispatch(Call::Send(send(&host, "kind-2", "看一下任务图的状态")))
        .await
        .expect("发送第二句");
    crate::common::wait_turns(&host, data.path(), 2).await;
    // Kind 那一轮跑了 task_status 读总任务：汇总阶段 + 子任务行。
    let status_result = crate::common::read_root_events(data.path())
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                error: None,
                ..
            } if message.tool_call_id.as_deref() == Some("call-graph-status") => {
                Some(message_text(message))
            }
            _ => None,
        })
        .expect("task_status 的结果");
    assert!(status_result.contains("任务图"), "汇总：{status_result}");
    assert!(
        status_result.contains("子任务数：2"),
        "汇总：{status_result}"
    );
    assert!(status_result.contains("先查行数"), "汇总：{status_result}");

    // task-read 读总任务：attempts 0、事件空（的空态）。
    let Response::TaskRead(read) = host
        .dispatch(Call::TaskRead(TaskReadRequest {
            task_id: root.id.as_str().to_owned(),
            before_seq: None,
        }))
        .await
        .expect("task-read 总任务")
    else {
        panic!("task-read 响应");
    };
    assert_eq!(read.task.attempts, 0);
    assert!(read.events.is_empty());
    assert_eq!(read.last_seq, 0);
}

/// 提议不合法（有环）：工具结果回可读原因，任务库不留半张图。
#[tokio::test]
async fn graph_propose_rejects_invalid_graph_with_reason() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    let bad = serde_json::json!({
        "title": "有环的图",
        "quote": "先查再改",
        "requirement": "r",
        "dod": "d",
        "subtasks": [
            {"title": "A", "requirement": "r", "dod": "d", "after": [1]},
            {"title": "B", "requirement": "r", "dod": "d", "after": [0]}
        ]
    })
    .to_string();
    router.push_kind(crate::common::tool_use_sse(
        1,
        "call-graph",
        "task_graph_propose",
        &bad,
    ));
    router.push_kind(crate::common::text_sse("好"));
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再改")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    let result = crate::common::read_root_events(data.path())
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, error, .. }
                if message.tool_call_id.as_deref() == Some("call-graph") =>
            {
                Some((message.is_error, message_text(message), error.is_some()))
            }
            _ => None,
        })
        .expect("工具结果");
    assert!(result.0 == Some(true) || result.2, "要报错：{result:?}");
    assert!(result.1.contains("有环"), "原因可读：{}", result.1);
    assert!(
        tasks_store(data.path())
            .task_ids()
            .expect("任务 id")
            .is_empty(),
        "不合格的图不落库"
    );
}

/// 暂不：图留着、不跑；没有 launch 效果，子任务都是待补足条件。
#[tokio::test]
async fn graph_dismiss_card_keeps_it_unconfirmed_and_launches_nothing() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.task_runtime()
        .expect("任务执行者")
        .set_pump_paused(true);
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再改")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
            && crate::common::read_root_events(data.path()).iter().any(|event| {
                matches!(&event.kind, session::EventKind::ToolResult { message, meta: Some(_), .. } if message.tool_call_id.as_deref() == Some("call-graph-card"))
            })
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    let Response::CardAct(response) = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Dismiss,
            option: None,

            note: None,
            report_id: None,
        }))
        .await
        .expect("暂不")
    else {
        panic!("card-act 响应");
    };
    assert!(
        response
            .decision
            .as_ref()
            .is_some_and(|decision| decision.phase == session::CardDecisionPhase::Dismissed)
    );
    let reloaded = graph_root(&store);
    assert!(
        reloaded
            .arrangement
            .as_ref()
            .expect("安排记录")
            .confirmed
            .is_none(),
        "暂不不算确认"
    );
    assert!(
        store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none(),
        "什么都不跑"
    );
    for child in children {
        assert_eq!(
            store.stage(&child.id).expect("阶段"),
            Stage::NeedsConditions
        );
    }
}

/// launch 入口统一挡：总任务不跑；没确认的图里的子任务不跑。
#[tokio::test]
async fn launch_gate_blocks_graph_root_and_unconfirmed_children() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再改")))
        .await
        .expect("发送");
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    // 手动替总任务和下游子任务各排一条 launch（模拟重启扫描/异常重放），launch 入口必须挡掉。
    store
        .enqueue_effect(
            tasks::EffectPayload::Launch {
                task_id: root.id.clone(),
            },
            1,
        )
        .expect("排总任务 launch");
    store
        .enqueue_effect(
            tasks::EffectPayload::Launch {
                task_id: children[1].id.clone(),
            },
            1,
        )
        .expect("排下游 launch");
    runtime.set_pump_paused(false);
    wait_until("launch 效果被收掉", || {
        tasks_store(data.path())
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .expect("取效果")
            .is_none()
    })
    .await;
    let store = tasks_store(data.path());
    assert!(
        store
            .task(&root.id)
            .expect("读任务")
            .expect("总任务")
            .runs
            .is_empty(),
        "总任务不跑"
    );
    assert!(
        store
            .task(&children[1].id)
            .expect("读任务")
            .expect("子任务")
            .runs
            .is_empty(),
        "没确认/上游没结果的下游不跑"
    );
}

/// A 的 ready 报告落库的同一事务放出 B；B 开跑时任务包里有「上游结果」，needs 上记交接。
#[tokio::test]
async fn graph_ready_releases_downstream_with_the_upstream_result() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let runtime = host.task_runtime().expect("任务执行者");
    runtime.set_pump_paused(true);

    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图并出卡", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).expect("子任务");
    // 确认：A 进 launch，B 等 A 的 ready。
    let Response::CardAct(response) = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),

            note: None,
            report_id: None,
        }))
        .await
        .expect("确认")
    else {
        panic!("card-act 响应");
    };
    assert!(response.decision.is_some());

    // ThinAgent 脚本按任务排：A 报 ready，B 报 ready；Kind 的后续轮多备几段文本。
    let report_a = crate::common::tool_use_sse(
        2,
        "call-a-report",
        "task_report",
        r#"{"state":"ready","summary":"查完了：notes.md 有 3 行"}"#,
    );
    let report_b = crate::common::tool_use_sse(
        2,
        "call-b-report",
        "task_report",
        r#"{"state":"ready","summary":"写好了：other.txt 里是 3 行"}"#,
    );
    let text = crate::common::text_sse("好");
    router.push_thin_for(children[0].id.as_str(), report_a.clone());
    router.push_thin_for(children[0].id.as_str(), text.clone());
    router.push_thin_for(children[1].id.as_str(), report_b);
    router.push_thin_for(children[1].id.as_str(), text);
    for _ in 0..6 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    runtime.set_pump_paused(false);

    wait_until("A 报 ready", || {
        tasks_store(data.path())
            .task(&children[0].id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Ready)
    })
    .await;
    wait_until("B 自动开跑", || {
        tasks_store(data.path())
            .task(&children[1].id)
            .ok()
            .flatten()
            .and_then(|task| {
                task.latest_attempt()
                    .and_then(|attempt| attempt.conversation.clone())
            })
            .is_some()
    })
    .await;

    let downstream = tasks_store(data.path())
        .task(&children[1].id)
        .expect("读任务")
        .expect("B 存在");
    let handoff = downstream.needs[0]
        .handoff
        .clone()
        .expect("B 开跑前解析出交接");
    assert!(handoff.summary.contains("3 行"));
    let upstream = tasks_store(data.path())
        .task(&children[0].id)
        .expect("读任务")
        .expect("A 存在");
    assert_eq!(
        handoff.report_id,
        upstream.latest_report().expect("A ready").id
    );

    // B 的任务包（assignment）里有「上游结果」一段：标题、要求版本、摘要。
    let conversation = downstream
        .latest_attempt()
        .and_then(|attempt| attempt.conversation.clone())
        .expect("B 的会话");
    let session_id = session::SessionId::parse(conversation.as_str()).expect("B 会话 id");
    // 固定块先落日志，等 assignment JSON 真正写进去再读。
    wait_until("B 的 assignment 落日志", || {
        let Ok(session) = session::SessionStore::open_read_only(data.path(), &session_id) else {
            return false;
        };
        session.events().iter().any(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => message
                .content
                .iter()
                .filter_map(|block| match block {
                    model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
                .parse::<serde_json::Value>()
                .map(|value| value["task_id"].is_string())
                .unwrap_or(false),
            _ => false,
        })
    })
    .await;
    let session =
        session::SessionStore::open_read_only(data.path(), &session_id).expect("B 的会话日志");
    let assignment = session
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => {
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                serde_json::from_str::<Value>(&text).ok()
            }
            _ => None,
        })
        .expect("B 的 assignment");
    let upstream_results = assignment["upstream_results"]
        .as_array()
        .expect("上游结果一段");
    assert_eq!(upstream_results.len(), 1);
    assert!(
        upstream_results[0]["title"]
            .as_str()
            .unwrap_or_default()
            .contains("先查行数")
    );
    assert_eq!(upstream_results[0]["requirement_version"], 1);
    assert!(
        upstream_results[0]["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("3 行")
    );

    wait_until("B 也报 ready、总任务待检查", || {
        tasks_store(data.path())
            .stage(&root.id)
            .map(|stage| stage == Stage::ReadyForReview)
            .unwrap_or(false)
    })
    .await;
}

#[tokio::test]
async fn gate_pass_flow_releases_guarded_downstream_after_the_user_verdict() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    // 判之前：B 是待检查，C 没开跑。
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::ReadyForReview);
    assert!(downstream.latest_attempt().is_none(), "C 还没开跑");
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );

    // 用户点通过：判定记下，C 被放出。
    let response = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判定");
    let Response::GateJudge(judged) = response else {
        panic!("gate-judge 响应");
    };
    assert!(!judged.changed, "不该是已经变了");
    assert!(!judged.duplicate);
    assert_eq!(judged.released, vec!["再写结果".to_owned()]);
    assert_eq!(
        judged.verdict.as_ref().map(|verdict| verdict.conclusion),
        Some(GateJudgeConclusion::Pass)
    );

    wait_until("C 开跑", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.latest_attempt().is_some())
    })
    .await;
    let store = tasks_store(data.path());
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::RequirementMet);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::RequirementMet);
    let b = store.task(&guarded.id).unwrap().unwrap();
    let c = store.task(&downstream.id).unwrap().unwrap();
    let b_report = b.latest_report().unwrap();
    assert_eq!(
        c.needs[0]
            .handoff
            .as_ref()
            .map(|handoff| handoff.report_id.clone()),
        Some(b_report.id.clone()),
        "C 交接的是判通过的那份"
    );

    // 同一份门报告再判：返回已有判定，不记第二次。
    let again = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::NeedsFix,
            note: None,
        }))
        .await
        .expect("再判");
    let Response::GateJudge(again) = again else {
        panic!("gate-judge 响应");
    };
    assert!(again.duplicate);
    assert_eq!(
        again.verdict.as_ref().map(|verdict| verdict.conclusion),
        Some(GateJudgeConclusion::Pass)
    );
    assert_eq!(
        tasks_store(data.path())
            .task(&gate.id)
            .unwrap()
            .unwrap()
            .verdicts
            .len(),
        1
    );
    let _ = root;
}

#[tokio::test]
async fn gate_blocked_keeps_guarded_task_and_downstream_untouched() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    // 门脚本换成 blocked：B 报回 ready 后门开跑，门报 blocked。
    router.enable_gate_kind();
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再核再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建门图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 4)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let (guarded, gate, downstream) = gate_children(&store, &root);
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b-report",
            "task_report",
            r#"{"state":"ready","summary":"第一次结果：3 行"}"#,
        ),
    );
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate-blocked",
            "task_report",
            r#"{"state":"blocked","summary":"缺原始链接，核不了","question":"要查什么？"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::CardAct(CardActRequest {
        call_id: "call-gate-card".to_owned(),
        verb: CardVerb::Confirm,
        option: Some("confirm".to_owned()),
        note: None,
        report_id: None,
    }))
    .await
    .expect("确认");
    wait_until("门报 blocked", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .and_then(|task| task.latest_report().map(|report| report.state))
            == Some(ReportState::Blocked)
    })
    .await;
    let store = tasks_store(data.path());
    let b = store.task(&guarded.id).unwrap().unwrap();
    assert_eq!(
        b.latest_report().map(|report| report.state),
        Some(ReportState::Ready),
        "B 不动"
    );
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    assert!(
        store
            .task(&downstream.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_none()
    );
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );
}

/// 判需修正 → relay 带原话 → task_continue → 新 Attempt 任务包含原话和核查发现 → 门自动重跑 → 通过。
#[tokio::test]
async fn gate_needs_fix_continue_and_auto_recheck_end_to_end() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    let (_root, guarded, downstream, gate_report) =
        reach_gate_ready(&host, &router, data.path()).await;
    let store = tasks_store(data.path());
    let gate = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .find(Task::is_gate)
        .expect("门任务");
    let note = "空标题的别直接放过，用域名加路径当键";
    // 用户判需修正（带原话）：relay 记下结论、原话和候选。
    let response = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: gate_report.as_str().to_owned(),
            conclusion: GateJudgeConclusion::NeedsFix,
            note: Some(note.to_owned()),
        }))
        .await
        .expect("判定需修正");
    let Response::GateJudge(judged) = response else {
        panic!("gate-judge 响应");
    };
    assert_eq!(
        judged
            .verdict
            .as_ref()
            .and_then(|verdict| verdict.note.clone()),
        Some(note.to_owned()),
        "原话原样存档"
    );
    wait_until("Kind 收到需修正 relay", || {
        crate::common::read_root_events(data.path())
            .iter()
            .any(|event| {
                let session::EventKind::UserMessage { message, .. } = &event.kind else {
                    return false;
                };
                let text: String = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                text.contains("gate_verdict")
                    && text.contains(note)
                    && text.contains("needs_fix")
                    && text.contains("milestone card comes back needing fixes")
                    && text.contains("program judges")
            })
    })
    .await;

    // Kind 用 task_continue 让被把关任务在原要求里接着做。
    router.push_thin_for(
        guarded.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-b2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次结果：5 行"}"#,
        ),
    );
    router.push_thin_for(guarded.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        gate.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-gate2-report",
            "task_report",
            r#"{"state":"ready","summary":"第二次核查：这次按域名加路径"}"#,
        ),
    );
    router.push_thin_for(gate.id.as_str(), crate::common::text_sse("好"));
    router.push_thin_for(
        downstream.id.as_str(),
        crate::common::tool_use_sse(
            2,
            "call-c-report",
            "task_report",
            r#"{"state":"ready","summary":"写好了"}"#,
        ),
    );
    router.push_thin_for(downstream.id.as_str(), crate::common::text_sse("好"));
    let continued = host
        .dispatch(Call::TaskContinue(service::TaskContinueRequest {
            task_id: guarded.id.as_str().to_owned(),
            note: Some(note.to_owned()),
        }))
        .await
        .expect("接着做");
    let Response::TaskContinue(continued) = continued else {
        panic!("task-continue 响应");
    };
    wait_until("B 第二次 Attempt 开跑", || {
        tasks_store(data.path())
            .task(&guarded.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_attempt()
                        .is_some_and(|attempt| attempt.conversation.is_some())
            })
    })
    .await;
    let store = tasks_store(data.path());
    let b = store.task(&guarded.id).unwrap().unwrap();
    let attempt = b.latest_attempt().unwrap();
    assert_eq!(attempt.id.0, continued.attempt_id);
    match &attempt.trigger {
        tasks::AttemptTrigger::KindContinue { cause, note } => {
            assert_eq!(
                note.as_deref(),
                Some("空标题的别直接放过，用域名加路径当键")
            );
            assert!(matches!(cause, tasks::ContinueCause::GateNeedsFix { .. }));
        }
        other => panic!("trigger 是 {other:?}"),
    }
    // 任务包里有「你要按这个改」：用户原话 + 核查发现。
    let user_texts = |data: &Path| -> Vec<String> {
        thin_events(data, b.latest_attempt().expect("B 最新 Attempt"))
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::UserMessage { message, .. } => Some(
                    message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    };
    wait_until("B 的二次任务包写进会话", || {
        user_texts(data.path())
            .iter()
            .any(|text| text.contains("你要按这个改"))
    })
    .await;
    let texts = user_texts(data.path());
    assert!(
        texts.iter().any(|text| text.contains("你要按这个改")
            && text.contains(note)
            && text.contains("核查发现：")
            && text.contains("空标题会漏判")),
        "任务包要带原话和核查发现：{texts:?}"
    );

    // B 再报 ready → 门自动重跑第 2 份候选；判通过后 C 才开。
    wait_until("门重跑第 2 份", || {
        tasks_store(data.path())
            .task(&gate.id)
            .ok()
            .flatten()
            .is_some_and(|task| {
                task.runs
                    .iter()
                    .map(|run| run.attempts.len())
                    .sum::<usize>()
                    == 2
                    && task
                        .latest_report()
                        .is_some_and(|report| report.summary.contains("第二次核查"))
            })
    })
    .await;
    let store = tasks_store(data.path());
    let gate_now = store.task(&gate.id).unwrap().unwrap();
    let second_gate_report = gate_now.latest_report().cloned().unwrap();
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    let pass = host
        .dispatch(Call::GateJudge(GateJudgeRequest {
            gate_task: gate.id.as_str().to_owned(),
            gate_report: second_gate_report.id.as_str().to_owned(),
            conclusion: GateJudgeConclusion::Pass,
            note: None,
        }))
        .await
        .expect("判通过");
    let Response::GateJudge(pass) = pass else {
        panic!("gate-judge 响应");
    };
    assert_eq!(pass.released, vec!["再写结果".to_owned()]);
    // 判通过后 C 进 launch 要占席位；先等门和第 2 份候选的 Attempt 都收尾，
    // 免得并发跑测试时「没空位」把 C 留成 Later（偶发）。
    wait_until("门和候选的 Attempt 都收尾", || {
        let store = tasks_store(data.path());
        [gate.id.clone(), guarded.id.clone()].iter().all(|id| {
            store
                .task(id)
                .ok()
                .flatten()
                .and_then(|task| task.latest_attempt().cloned())
                .is_none_or(|attempt| attempt.ended_at.is_some())
        })
    })
    .await;
    wait_until("C 开跑", || {
        tasks_store(data.path())
            .task(&downstream.id)
            .ok()
            .flatten()
            .is_some_and(|task| task.latest_attempt().is_some())
    })
    .await;
    assert_eq!(
        tasks_store(data.path()).stage(&guarded.id).unwrap(),
        Stage::RequirementMet
    );
}

/// 点暂不 → 图取消（Dismissed），一项不跑；再点确认已变。
#[tokio::test]
async fn graph_dismissed_on_card_cancels_the_graph() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    let children = store.children(&root.id).unwrap();
    let dismissed = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Dismiss,
            option: None,
            note: None,
            report_id: None,
        }))
        .await
        .expect("暂不");
    let Response::CardAct(dismissed) = dismissed else {
        panic!("card-act 响应");
    };
    assert!(dismissed.decision.is_some());
    let store = tasks_store(data.path());
    let cancelled = store
        .task(&root.id)
        .unwrap()
        .unwrap()
        .arrangement
        .unwrap()
        .cancelled
        .expect("图要有取消记录");
    assert_eq!(cancelled.reason, tasks::CancellationReason::Dismissed);
    for child in &children {
        assert!(
            store
                .task(&child.id)
                .unwrap()
                .unwrap()
                .latest_attempt()
                .is_none()
        );
        assert_eq!(store.stage(&child.id).unwrap(), Stage::NeedsConditions);
        assert!(
            !store
                .effect_pending(EffectKind::Launch, child.id.as_str())
                .unwrap()
        );
    }
    // 再点确认：返回已有的暂不决定，不执行、不启动。
    let again = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-graph-card".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("再点");
    let Response::CardAct(again) = again else {
        panic!("card-act 响应");
    };
    assert_eq!(
        again.decision.as_ref().map(|decision| decision.phase),
        Some(session::CardDecisionPhase::Dismissed)
    );
    assert!(
        tasks_store(data.path())
            .children(&root.id)
            .unwrap()
            .iter()
            .all(|child| child.latest_attempt().is_none())
    );
}

/// Kind 一轮提了图却没上卡 → 下一轮开始前按 NoCard 取消。
#[tokio::test]
async fn uncarded_graph_is_cancelled_as_no_card() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_nocard_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("建图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let root = graph_root(&store);
    assert!(
        store
            .task(&root.id)
            .unwrap()
            .unwrap()
            .arrangement
            .unwrap()
            .cancelled
            .is_none()
    );
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    host.dispatch(Call::Send(send(&host, "kind-2", "再问一句")))
        .await
        .expect("第二轮");
    wait_until("NoCard 取消", || {
        tasks_store(data.path())
            .task(&root.id)
            .ok()
            .flatten()
            .and_then(|task| task.arrangement)
            .and_then(|arrangement| arrangement.cancelled)
            .is_some_and(|cancelled| cancelled.reason == tasks::CancellationReason::NoCard)
    })
    .await;
}

/// 新卡接受后，之前没点过的旧图按 Replaced 取消。
#[tokio::test]
async fn replacing_card_cancels_the_earlier_graph() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let router = Router::new();
    mount(&server, &router).await;
    router.enable_two_graph_kind();
    let host = open_graph_host(&server, data.path(), cwd.path()).await;
    host.dispatch(Call::Send(send(&host, "kind-1", "先查再写")))
        .await
        .expect("发送");
    crate::common::wait_turns(&host, data.path(), 1).await;
    wait_until("第一张图", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 3)
            .unwrap_or(false)
    })
    .await;
    // 下一轮再提一张图一张卡（同一轮只能出一张卡）。
    for _ in 0..10 {
        router.push_kind(crate::common::text_sse("收到"));
    }
    router.enable_two_graph_second();
    host.dispatch(Call::Send(send(&host, "kind-2", "换一版")))
        .await
        .expect("第二轮");
    crate::common::wait_turns(&host, data.path(), 2).await;
    wait_until("两张图都建好", || {
        tasks_store(data.path())
            .task_ids()
            .map(|ids| ids.len() == 6)
            .unwrap_or(false)
    })
    .await;
    let store = tasks_store(data.path());
    let roots = store
        .task_ids()
        .unwrap()
        .into_iter()
        .filter_map(|id| store.task(&id).ok().flatten())
        .filter(Task::is_graph_root)
        .collect::<Vec<_>>();
    let first_root = roots
        .iter()
        .find(|task| task.title == "第一张图")
        .cloned()
        .expect("第一张图");
    let second_root = roots
        .iter()
        .find(|task| task.title == "第二张图")
        .cloned()
        .expect("第二张图");
    let confirmed = host
        .dispatch(Call::CardAct(CardActRequest {
            call_id: "call-two-card-2".to_owned(),
            verb: CardVerb::Confirm,
            option: Some("confirm".to_owned()),
            note: None,
            report_id: None,
        }))
        .await
        .expect("确认第二张卡");
    let Response::CardAct(confirmed) = confirmed else {
        panic!("card-act 响应");
    };
    assert!(confirmed.decision.is_some());
    let store = tasks_store(data.path());
    let second = store.task(&second_root.id).unwrap().unwrap();
    assert!(
        second
            .arrangement
            .as_ref()
            .and_then(|arrangement| arrangement.confirmed.as_ref())
            .is_some()
    );
    let first = store.task(&first_root.id).unwrap().unwrap();
    assert_eq!(
        first
            .arrangement
            .unwrap()
            .cancelled
            .expect("第一张图要被替换取消")
            .reason,
        tasks::CancellationReason::Replaced
    );
    assert!(
        store
            .children(&first_root.id)
            .unwrap()
            .iter()
            .all(|child| child.latest_attempt().is_none())
    );
}
