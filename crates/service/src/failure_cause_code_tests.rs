use super::*;
use crate::events::failure_cause;

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

#[test]
fn failure_cause_keyring_uses_structured_code() {
    assert_eq!(
        failure_cause(&turn_end("KEYRING", "任何消息")),
        Some(FailureCause::Keyring),
        "KEYRING 码必须映射 Keyring，不能看消息文本"
    );
    assert_eq!(
        failure_cause(&turn_end(
            "MISSING_CREDENTIAL",
            "Keyring 无法签发：钥匙文件不在"
        )),
        Some(FailureCause::Credential),
        "消息里有 Keyring 也不能改变码的分类"
    );
}

#[test]
fn failure_cause_masked_uses_model_disabled_code() {
    assert_eq!(
        failure_cause(&turn_end(
            "MODEL_DISABLED",
            "deepseek-flash 已在设置里屏蔽（规则「deepseek/deepseek-flash」）"
        )),
        Some(FailureCause::Masked),
        "MODEL_DISABLED 必须映射 masked（失败块给换模型/去设置）"
    );
}
