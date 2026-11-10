//! 工具策略：拒绝会让一轮停住等待的工具调用。
//!
//! 照 DSH 的 noWait（`dsh-superagent-prototype/src/app/sessions.ts:73-87`）：Kind 不等待，
//! 上报、结果和后台任务的结束都会自己作为消息送到。命中拒绝规则的调用已在统一工具入口
//! 记过 tool/call，这里只产生错误 tool/result，不执行工具本身。

/// 命中的拒绝语（照 DSH 的 NO_WAIT）：等的东西会自己送到，结束这一轮。
pub const NO_WAIT: &str = "Reports, follow_up notices, new candidates and a background job's end with its output arrive by themselves as messages; end the turn instead of waiting for them.";

/// bash 没给 timeoutMs、也没要求后台时的提醒（照 DSH 的 DEFAULT_WAIT，等待时长改成 AIOS
/// bash 的默认 120 秒）：给不超过 30000 的超时，或者转后台。
pub const DEFAULT_WAIT: &str = "This shell waits up to 120 seconds by default: pass timeoutMs of at most 30000, or run_in_background, as a waiting step holds back the user's next message.";

/// 一次调用会不会等待；命中返回拒绝语。
#[allow(dead_code)]
pub(crate) fn no_wait_reason(_name: &str, _arguments: &str) -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_WAIT, NO_WAIT, no_wait_reason};

    #[track_caller]
    fn check(arguments: &str, expected: Option<&str>) {
        assert_eq!(
            no_wait_reason("bash", arguments),
            expected,
            "bash {arguments}"
        );
    }

    #[track_caller]
    fn check_job_output(arguments: &str, expected: Option<&str>) {
        assert_eq!(
            no_wait_reason("job_output", arguments),
            expected,
            "job_output {arguments}"
        );
    }

    #[test]
    fn rejects_sleep_at_a_command_position() {
        check(
            r#"{"description":"wait","command":"sleep 5"}"#,
            Some(NO_WAIT),
        );
        check(
            r#"{"description":"wait","command":"echo hi; sleep 5"}"#,
            Some(NO_WAIT),
        );
        check(
            r#"{"description":"wait","command":"echo hi\nsleep 5"}"#,
            Some(NO_WAIT),
        );
        check(
            r#"{"description":"wait","command":"if sleep 1; then :; fi"}"#,
            Some(NO_WAIT),
        );
        check(
            r#"{"description":"wait","command":"/bin/sleep 1"}"#,
            Some(NO_WAIT),
        );
    }

    #[test]
    fn does_not_treat_sleep_as_a_command_outside_a_command_position() {
        check(
            r#"{"description":"x","command":"echo sleep 5","timeoutMs":20000}"#,
            None,
        );
        check(
            r#"{"description":"x","command":"sleepy 5","timeoutMs":20000}"#,
            None,
        );
        check(
            r#"{"description":"x","command":"grep sleep file","timeoutMs":20000}"#,
            None,
        );
        check(
            r#"{"description":"x","command":"ls /tmp/sleep","timeoutMs":20000}"#,
            None,
        );
    }

    #[test]
    fn rejects_bash_timeout_above_thirty_seconds() {
        check(
            r#"{"description":"x","command":"echo hi","timeoutMs":30001}"#,
            Some(NO_WAIT),
        );
        check(
            r#"{"description":"x","command":"echo hi","timeoutMs":30000}"#,
            None,
        );
    }

    #[test]
    fn allows_a_twenty_second_timeout() {
        check(
            r#"{"description":"x","command":"echo hi","timeoutMs":20000}"#,
            None,
        );
    }

    #[test]
    fn rejects_a_foreground_bash_without_a_timeout() {
        check(
            r#"{"description":"x","command":"echo hi"}"#,
            Some(DEFAULT_WAIT),
        );
        check(
            r#"{"description":"x","command":"echo hi","run_in_background":true}"#,
            None,
        );
    }

    #[test]
    fn rejects_a_long_or_default_job_wait() {
        check_job_output(r#"{"job_id":"job-1","wait":true}"#, Some(NO_WAIT));
        check_job_output(
            r#"{"job_id":"job-1","wait":true,"timeout_ms":5001}"#,
            Some(NO_WAIT),
        );
        check_job_output(r#"{"job_id":"job-1","wait":true,"timeout_ms":5000}"#, None);
        check_job_output(r#"{"job_id":"job-1","wait":false}"#, None);
    }
}
