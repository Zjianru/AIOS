//! T007.2 红测试：根会话不可用也照常起服务；ready 报状态+路径/行号；方法拒绝；recheck 修好接管。

use std::fs;
use std::path::Path;

use agent::AgentConfig;
use model::DeepSeekConfig;
use service::{
    Call, ContextUsage, PROTOCOL_VERSION, RecheckRequest, RootHost, RootSessionOptions, RootState,
    RootUnavailableKind, ServiceError, StatusRequest,
};
use tempfile::tempdir;

const ABSENT_SESSION: &str = "00000000-0000-4000-8000-000000000000";

fn options(data_dir: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
        model: DeepSeekConfig {
            api_key: "test-key".to_owned(),
            ..DeepSeekConfig::default()
        },
        config: AgentConfig::default(),
        credentials: None,
        identity: None,
        model_factory: None,
    }
}

fn write_record(data_dir: &Path, session_id: &str) {
    fs::create_dir_all(data_dir).unwrap();
    fs::write(
        data_dir.join(service::ROOT_STATE_FILE),
        format!(r#"{{"version":1,"sessionId":"{session_id}"}}"#),
    )
    .unwrap();
}

#[tokio::test]
async fn missing_root_log_reports_lost_with_path_and_rejects_methods() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    write_record(&data, ABSENT_SESSION);
    let host = RootHost::open(options(&data), 1_000).await;
    let info = host.info();
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(
        info.context,
        ContextUsage {
            used: 0,
            window: 1_000
        }
    );
    let RootState::Unavailable { kind, path, .. } = &info.root else {
        panic!("根会话缺失必须是 unavailable：{:?}", info.root);
    };
    assert_eq!(*kind, RootUnavailableKind::Lost);
    assert!(
        path.as_deref()
            .is_some_and(|path| path.contains(ABSENT_SESSION)),
        "ready 要带缺失日志的路径：{path:?}"
    );
    assert!(matches!(
        host.dispatch(Call::Status(StatusRequest::default())).await,
        Err(ServiceError::RootUnavailable(_))
    ));
    assert!(matches!(
        host.follow(service::FollowRequest::default()).await,
        Err(ServiceError::RootUnavailable(_))
    ));
}

#[tokio::test]
async fn corrupt_root_state_reports_corrupt_with_path_and_line() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join(service::ROOT_STATE_FILE), b"{\"version\": 1, ").unwrap();
    let host = RootHost::open(options(&data), 1_000).await;
    let RootState::Unavailable {
        kind, path, line, ..
    } = &host.info().root
    else {
        panic!("状态文件坏必须是 unavailable：{:?}", host.info().root);
    };
    assert_eq!(*kind, RootUnavailableKind::Corrupt);
    assert!(
        path.as_deref()
            .is_some_and(|path| path.contains("kind-root.json"))
    );
    assert_eq!(*line, Some(1), "坏 JSON 要带行号");
}

#[tokio::test]
async fn recheck_recovers_after_root_state_is_restored() {
    let dir = tempdir().unwrap();
    let data = dir.path().join("data");
    // 先正常打开，建出根会话并记下 id。
    let initial = RootHost::open(options(&data), 1_000).await;
    assert_eq!(initial.info().root, RootState::Ok);
    let session_id = initial.info().session_id.expect("根会话 id");
    drop(initial);
    // 改成指向不存在的会话：服务照常起，状态是 lost。
    write_record(&data, ABSENT_SESSION);
    let host = RootHost::open(options(&data), 1_000).await;
    assert!(matches!(
        host.info().root,
        RootState::Unavailable {
            kind: RootUnavailableKind::Lost,
            ..
        }
    ));
    // 修好状态文件后 recheck 接管。
    write_record(&data, &session_id);
    let response = host
        .dispatch(Call::Recheck(RecheckRequest::default()))
        .await
        .expect("recheck 不能被拒绝");
    let service::Response::Recheck(recheck) = response else {
        panic!("recheck 必须回 RecheckResponse");
    };
    assert_eq!(recheck.root, RootState::Ok);
    assert_eq!(host.info().root, RootState::Ok);
    // 修好后普通方法必须恢复可用（回归：install 必须把 ServiceHost 放进 inner）。
    let status = host
        .dispatch(Call::Status(StatusRequest::default()))
        .await
        .expect("recheck 后 status 必须恢复");
    let service::Response::Status(status) = status else {
        panic!("status 应回 StatusResponse");
    };
    assert_eq!(status.object.session_id, session_id);
}

#[test]
fn writer_held_classifies_busy() {
    let state = RootState::from_root_error(&service::RootError::WriterHeld {
        session_id: "sess-1".to_owned(),
    });
    assert!(matches!(
        state,
        RootState::Unavailable {
            kind: RootUnavailableKind::Busy,
            ..
        }
    ));
}
