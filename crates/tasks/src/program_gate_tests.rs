//! T057：程序门数据/旧行兼容/命令校验。

use crate::{
    GateCommand, GateConclusion, MilestoneGate, TaskError, VerdictSource, validate_gate_commands,
};

#[test]
fn old_gate_and_verdict_json_read_as_user_judged_zero_version() {
    let gate: MilestoneGate = serde_json::from_str("{}").unwrap();
    assert!(!gate.is_program_gate());
    assert_eq!(gate.commands_version, 0);
    assert!(gate.commands.is_empty());

    let verdict: crate::GateVerdict = serde_json::from_str(
        r#"{"gate_report":"r1","candidate":{"report_id":"c1","requirement_version":1},"conclusion":"pass","at":10}"#,
    )
    .unwrap();
    assert_eq!(verdict.judged_by, VerdictSource::User);
    assert_eq!(verdict.commands_version, 0);
    assert_eq!(verdict.conclusion, GateConclusion::Pass);
}

#[test]
fn gate_command_validation() {
    let command = |text: &str, timeout_ms: Option<u64>| GateCommand {
        command: text.to_owned(),
        timeout_ms,
    };
    assert!(validate_gate_commands(&[]).is_ok());
    assert!(matches!(
        validate_gate_commands(&[command("  ", None)]),
        Err(TaskError::EmptyGateCommand)
    ));
    let long = "长".repeat(501);
    assert!(matches!(
        validate_gate_commands(&[command(&long, None)]),
        Err(TaskError::GateCommandTooLong(501))
    ));
    assert!(matches!(
        validate_gate_commands(&[command("true", None), command(" true ", None)]),
        Err(TaskError::DuplicateGateCommand(_))
    ));
    assert!(matches!(
        validate_gate_commands(&[command("true", Some(600_001))]),
        Err(TaskError::GateCommandTimeoutInvalid(600_001))
    ));
    assert!(matches!(
        validate_gate_commands(&[command("true", Some(0))]),
        Err(TaskError::GateCommandTimeoutInvalid(0))
    ));
    assert!(validate_gate_commands(&[command("true", Some(600_000))]).is_ok());
}
