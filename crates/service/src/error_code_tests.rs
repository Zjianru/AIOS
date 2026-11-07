use std::path::PathBuf;

use super::*;
use crate::events::failure_cause;

fn variants() -> Vec<(&'static str, ServiceError)> {
    vec![
        (
            "agent/internal",
            ServiceError::Agent(agent::AgentError::Compaction("x".to_owned())),
        ),
        (
            "question/answer-rejected",
            ServiceError::AnswerRejected("q1".to_owned()),
        ),
        (
            "root/unavailable",
            ServiceError::RootUnavailable(RootState::Ok),
        ),
        (
            "keyring/key-file-missing",
            ServiceError::Credentials(credentials::KeyringError::KeyFileMissing(PathBuf::from(
                "/k",
            ))),
        ),
        (
            "keyring/undecryptable",
            ServiceError::Credentials(credentials::KeyringError::Crypto("x".to_owned())),
        ),
        (
            "keyring/corrupt",
            ServiceError::Credentials(credentials::KeyringError::Corrupt("x".to_owned())),
        ),
        (
            "keyring/io",
            ServiceError::Credentials(credentials::KeyringError::Io(std::io::Error::other("x"))),
        ),
        (
            "connection/missing",
            ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                "deepseek".to_owned(),
            )),
        ),
        (
            "connection/invalid",
            ServiceError::Credentials(credentials::KeyringError::InvalidConnection("x".to_owned())),
        ),
        ("settings/unavailable", ServiceError::SettingsOnBareHost),
        (
            "settings/unreadable",
            ServiceError::SettingsUnreadable("bad".to_owned()),
        ),
        (
            "request/invalid-path",
            ServiceError::InvalidPath("bad".to_owned()),
        ),
        (
            "attachment/invalid",
            ServiceError::AttachmentInvalid("x".to_owned()),
        ),
        ("attachment/not-text", ServiceError::AttachmentNotText),
        ("request/invalid-message", ServiceError::MessageEmpty),
    ]
}

#[test]
fn service_error_codes_cover_every_variant() {
    let pairs = variants();
    for (expected, error) in &pairs {
        assert_eq!(error.code().as_str(), *expected, "变体的码");
    }
    let mut used: Vec<&str> = pairs.iter().map(|(code, _)| *code).collect();
    used.sort_unstable();
    used.dedup();
    assert_eq!(ServiceErrorCode::ALL.len(), 16, "码数 16（加了 settings/unreadable）");
    assert!(
        ServiceErrorCode::ALL
            .iter()
            .any(|code| code.as_str() == "request/invalid-json"),
        "invalid-json 在唯一码表里"
    );
    let mut all: Vec<&str> = ServiceErrorCode::ALL
        .iter()
        .map(|code| code.as_str())
        .filter(|code| *code != "request/invalid-json")
        .collect();
    all.sort_unstable();
    assert_eq!(
        used, all,
        "码表与 ServiceError 变体必须一一对应（多/少都失败；invalid-json 由 web 层用）"
    );
}

#[test]
fn invalid_json_code_is_in_the_single_code_table() {
    assert_eq!(
        ServiceErrorCode::ALL.len(),
        16,
        "码数 16（settings/unreadable 进面）"
    );
    assert!(
        ServiceErrorCode::ALL
            .iter()
            .any(|code| code.as_str() == "request/invalid-json"),
        "request/invalid-json 必须进唯一码表 ALL"
    );
}

#[test]
fn page_error_table_covers_service_and_model_codes() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/data/errors.ts");
    let table = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("页面错误表 {} 读不到：{error}", path.display()));
    for code in ServiceErrorCode::ALL {
        assert!(
            table.contains(code.as_str()),
            "页面对照表缺 {} 这一行",
            code.as_str()
        );
    }
    // 日志里的模型错误码（spec 测试清单 2）。
    for code in [
        "AUTH",
        "MISSING_CREDENTIAL",
        "KEYRING",
        "QUOTA",
        "RATE_LIMIT",
        "CONTEXT_WINDOW_EXCEEDED",
        "INVALID_REQUEST",
        "SERVER",
        "TRANSPORT",
        "TIMEOUT",
        "STREAM_CLOSED",
        "MALFORMED_RESPONSE",
        "EMPTY_RESPONSE",
        "UNSUPPORTED_CONTENT",
        "UNSUPPORTED_REASONING_EFFORT",
        "ABORTED",
        "MODEL_DISABLED",
    ] {
        assert!(table.contains(code), "页面对照表缺模型码 {code} 这一行");
    }
}

#[test]
fn service_error_body_has_code_message_and_root_details() {
    let root_error = ServiceError::RootUnavailable(RootState::Unavailable {
        kind: RootUnavailableKind::Other,
        path: Some("/x".to_owned()),
        line: Some(3),
        message: "x".to_owned(),
    });
    let body = ServiceErrorBody::from_service_error(&root_error);
    let json = serde_json::to_value(&body).expect("serialize");
    assert_eq!(json["code"], "root/unavailable");
    assert!(
        json["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    assert_eq!(json["details"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(
        json["details"].get("root").is_some(),
        "details.root：{json}"
    );
    assert!(json.get("error").is_none(), "不再有旧 error 字段：{json}");
    let other =
        ServiceErrorBody::from_service_error(&ServiceError::AnswerRejected("q1".to_owned()));
    let other_json = serde_json::to_value(&other).expect("serialize");
    assert_eq!(other_json["code"], "question/answer-rejected");
    assert!(
        other_json.get("details").is_none(),
        "无 detail 时不出现该字段：{other_json}"
    );
    assert!(other_json.get("error").is_none());
}

#[test]
fn quota_error_is_credential_cause() {
    assert_eq!(
        failure_cause(&turn_end("QUOTA", "额度用完")),
        Some(FailureCause::Credential),
        "QUOTA 归凭证类"
    );
}

fn turn_end(code: &str, message: &str) -> session::EventKind {
    session::EventKind::TurnEnd {
        turn: 1,
        reason: session::TurnEndReason::Error {
            error: session::LlmFailure {
                message: message.to_owned(),
                code: code.to_owned(),
            },
        },
    }
}
