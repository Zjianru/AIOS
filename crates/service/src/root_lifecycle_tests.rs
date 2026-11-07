use crate::protocol::{RootState, RootUnavailableKind};
use crate::root_lifecycle::initial_selection_state;

#[test]
fn initial_selection_failure_is_visible_with_reason() {
    let error = agent::AgentError::Compaction("会话日志写不进去".to_owned());
    let state = initial_selection_state(&error);
    let RootState::Unavailable { kind, message, .. } = state else {
        panic!("初始选择失败要报 root 不可用，实际 {state:?}");
    };
    assert_eq!(kind, RootUnavailableKind::Other);
    assert!(message.contains("初始模型选择"), "message={message}");
    assert!(message.contains(&error.to_string()), "message={message}");
}
