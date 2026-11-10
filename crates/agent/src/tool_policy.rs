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
pub(crate) fn no_wait_reason(name: &str, arguments: &str) -> Option<&'static str> {
    let value: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let args = value.as_object()?;
    match name {
        "bash" => {
            let sleeps = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(sleep_at_command_position);
            let timeout = args.get("timeoutMs");
            let too_long = timeout
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|milliseconds| milliseconds > 30_000.0);
            if sleeps || too_long {
                return Some(NO_WAIT);
            }
            let background = args
                .get("run_in_background")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            if timeout.is_none() && !background {
                return Some(DEFAULT_WAIT);
            }
            None
        }
        "job_output" => {
            if args.get("wait").and_then(serde_json::Value::as_bool) != Some(true) {
                return None;
            }
            let timeout = args.get("timeout_ms");
            let too_long = timeout
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|milliseconds| milliseconds > 5_000.0);
            if timeout.is_none() || too_long {
                return Some(NO_WAIT);
            }
            None
        }
        _ => None,
    }
}

/// 命令位置上的 `sleep`（照 DSH 的 SLEEP 正则）：行首、命令分隔符之后、
/// 关键字之后，允许前面带路径；`echo sleep` 这类参数位置不算。
fn sleep_at_command_position(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut offset = 0;
    while let Some(found) = command[offset..].find("sleep") {
        let start = offset + found;
        let end = start + "sleep".len();
        if !bytes.get(end).is_some_and(|byte| is_word_byte(*byte))
            && is_command_position(bytes, start)
        {
            return true;
        }
        offset = end;
    }
    false
}

fn is_command_position(bytes: &[u8], sleep_start: usize) -> bool {
    // 命令前可以带路径（DSH 的 `(?:\S*\/)?` 分组）；带路径时把整段 token 当作候选命令。
    let mut start = sleep_start;
    if sleep_start > 0 && bytes[sleep_start - 1] == b'/' {
        while start > 0
            && !bytes[start - 1].is_ascii_whitespace()
            && !is_separator(bytes[start - 1])
        {
            start -= 1;
        }
    }
    // 命令位置必须紧跟行首、空白或命令分隔符；参数中间的 `x=sleep` 不算。
    if start > 0 && !bytes[start - 1].is_ascii_whitespace() && !is_separator(bytes[start - 1]) {
        return false;
    }
    // 跳过空白；空白里有换行时，换行本身就是命令分隔符。
    let mut cursor = start;
    let mut newline = false;
    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
        newline |= bytes[cursor - 1] == b'\n';
        cursor -= 1;
    }
    if newline || cursor == 0 {
        return true;
    }
    if is_separator(bytes[cursor - 1]) {
        return true;
    }
    // 关键字后面跟空白：do/then/else/elif/if/while/until/time/exec/nohup/command/env。
    let mut word_start = cursor;
    while word_start > 0 && is_word_byte(bytes[word_start - 1]) {
        word_start -= 1;
    }
    COMMAND_KEYWORDS
        .iter()
        .any(|keyword| keyword.as_bytes() == &bytes[word_start..cursor])
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_separator(byte: u8) -> bool {
    matches!(byte, b';' | b'&' | b'|' | b'(' | b'{' | b'!' | b'`' | b'\n')
}

const COMMAND_KEYWORDS: [&str; 12] = [
    "do", "then", "else", "elif", "if", "while", "until", "time", "exec", "nohup", "command", "env",
];

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
