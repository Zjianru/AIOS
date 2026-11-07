use super::*;
use crate::events::{context_used, failure_cause};

fn turn_end(code: &str) -> session::EventKind {
    session::EventKind::TurnEnd {
        turn: 1,
        reason: session::TurnEndReason::Error {
            error: session::LlmFailure {
                message: "boom".to_owned(),
                code: code.to_owned(),
            },
        },
    }
}

#[test]
fn failure_cause_classifies_credential_and_model() {
    assert_eq!(
        failure_cause(&turn_end("AUTH")),
        Some(FailureCause::Credential)
    );
    assert_eq!(
        failure_cause(&turn_end("MISSING_CREDENTIAL")),
        Some(FailureCause::Credential)
    );
    assert_eq!(
        failure_cause(&turn_end("SERVER")),
        Some(FailureCause::Model)
    );
    let keyring = session::EventKind::TurnEnd {
        turn: 1,
        reason: session::TurnEndReason::Error {
            error: session::LlmFailure {
                message: "Keyring 无法签发：钥匙文件不在".to_owned(),
                code: "KEYRING".to_owned(),
            },
        },
    };
    assert_eq!(failure_cause(&keyring), Some(FailureCause::Keyring));
    assert_eq!(
        failure_cause(&session::EventKind::StepStart { turn: 1, step: 1 }),
        None
    );
}

#[test]
fn context_used_reads_prompt_tokens_from_usage() {
    let event = ServiceEvent {
        seq: 1,
        time: 1,
        event_type: "assistant/message".to_owned(),
        data: serde_json::json!({"usage": {"inputTokens": 100, "cacheReadTokens": 20, "cacheWriteTokens": 5, "outputTokens": 9}}),
        cause: None,
    };
    assert_eq!(context_used(&event), Some(125));
    let non_message = ServiceEvent {
        seq: 2,
        time: 2,
        event_type: "turn/start".to_owned(),
        data: serde_json::json!({}),
        cause: None,
    };
    assert_eq!(context_used(&non_message), None);
}
