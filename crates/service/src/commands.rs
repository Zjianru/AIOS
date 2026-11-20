//! 从会话日志读 bash 命令事实（程序名、失败、连续同命令同结果）。
//!
//! 不新建采集：只读 tool/call 的 arguments.command 和 tool/result 的正文/error。

use std::collections::HashMap;
use std::path::Path;

use model::ContentBlock;
use serde::{Deserialize, Serialize};
use session::EventKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandFact {
    pub call_id: String,
    pub command: String,
    pub program: String,
    pub failed: bool,
    pub result: String,
    /// 结果事件时间（unix 毫秒）：监控按用户时区落天用。
    pub time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatedCommand {
    pub command: String,
    pub program: String,
    pub result: String,
    pub count: u64,
}

/// 一次会话里的 bash 命令事实，按工具结果顺序。
pub fn command_facts(events: &[session::Event]) -> Vec<CommandFact> {
    let mut calls: HashMap<String, String> = HashMap::new();
    let mut facts = Vec::new();
    for event in events {
        match &event.kind {
            EventKind::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } if name == "bash" => {
                let command = serde_json::from_str::<serde_json::Value>(arguments)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("command")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                calls.insert(call_id.clone(), command);
            }
            EventKind::ToolResult { message, error, .. } => {
                let Some(id) = message.tool_call_id.as_deref() else {
                    continue;
                };
                let Some(command) = calls.get(id).cloned() else {
                    continue;
                };
                let result = message
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                facts.push(CommandFact {
                    call_id: id.to_owned(),
                    program: program_name(&command),
                    command,
                    failed: command_failed(error.is_some(), &result),
                    result,
                    time: event.time,
                });
            }
            _ => {}
        }
    }
    facts
}

/// 同一已结束命令连续出现、结果原样相同的 N>=2 事实；夹别的 bash 重置，夹读/回复不打断。
pub fn repeated_commands(events: &[session::Event]) -> Vec<RepeatedCommand> {
    let mut repeated = Vec::new();
    let mut current: Option<(String, String, u64)> = None;
    for fact in command_facts(events) {
        match &mut current {
            Some((command, result, count))
                if *command == fact.command && *result == fact.result =>
            {
                *count += 1;
            }
            _ => {
                if let Some((command, result, count)) = current.take()
                    && count >= 2
                {
                    repeated.push(RepeatedCommand {
                        program: program_name(&command),
                        command,
                        result,
                        count,
                    });
                }
                current = Some((fact.command, fact.result, 1));
            }
        }
    }
    if let Some((command, result, count)) = current
        && count >= 2
    {
        repeated.push(RepeatedCommand {
            program: program_name(&command),
            command,
            result,
            count,
        });
    }
    repeated
}

/// shell-words 切不开时退回整串第一个词；先去掉 VAR=x 前缀。
pub fn program_name(command: &str) -> String {
    let words = shell_words::split(command).unwrap_or_else(|_| {
        command
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let program = words
        .iter()
        .find(|word| !is_assignment(word))
        .or_else(|| words.first())
        .map(String::as_str)
        .unwrap_or_default();
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(program)
        .to_owned()
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// 失败认 tool/result.error 与正文末尾的退出/信号标记；没有标记按成功。
pub fn command_failed(error: bool, result: &str) -> bool {
    if error {
        return true;
    }
    let last = result
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if last.contains("[killed by signal:") {
        return true;
    }
    last.rsplit_once("[exit code:")
        .and_then(|(_, rest)| rest.trim().trim_end_matches(']').trim().parse::<i32>().ok())
        .is_some_and(|code| code != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_name_uses_shell_words_and_basename() {
        assert_eq!(program_name("cd /tmp && cargo test -q"), "cd");
        assert_eq!(program_name("PATH=/bin:$PATH cargo test"), "cargo");
        assert_eq!(program_name("/usr/bin/python3 -c 'print(1)'"), "python3");
        assert_eq!(program_name("FOO=1 ./run.sh a b"), "run.sh");
    }

    #[test]
    fn failure_reads_error_and_markers() {
        assert!(!command_failed(false, "ok"));
        assert!(command_failed(true, "ok"));
        assert!(command_failed(false, "bad\n[exit code: 1]"));
        assert!(!command_failed(false, "ok\n[exit code: 0]"));
        assert!(command_failed(false, "[killed by signal: 9]"));
    }
    fn bash(call: &str, command: &str) -> session::Event {
        session::Event::new(
            1,
            1,
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: call.to_owned(),
                name: "bash".to_owned(),
                arguments: serde_json::json!({"command": command}).to_string(),
            },
        )
    }

    fn read(call: &str) -> session::Event {
        session::Event::new(
            1,
            1,
            EventKind::ToolCall {
                turn: 1,
                step: 1,
                call_id: call.to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "foo"}).to_string(),
            },
        )
    }

    fn result(call: &str, text: &str, error: bool) -> session::Event {
        session::Event::new(
            2,
            2,
            EventKind::ToolResult {
                turn: 1,
                step: 1,
                message: model::Message::tool_result(call, text, error),
                error: error.then(|| session::ToolError {
                    name: "bash".to_owned(),
                    code: "EXIT".to_owned(),
                }),
                meta: None,
            },
        )
    }

    #[test]
    fn repeats_ignore_reads_and_reset_on_other_bash() {
        let events = vec![
            bash("c1", "cargo test -q"),
            result("c1", "ok", false),
            read("r1"),
            result("r1", "file.rs:1", false),
            bash("c2", "cargo test -q"),
            result("c2", "ok", false),
            bash("c3", "cargo test -q"),
            result("c3", "ok\n[exit code: 1]", false),
            bash("c4", "cargo test -q"),
            result("c4", "ok", false),
            bash("c5", "cargo test -q"),
            result("c5", "ok", false),
        ];
        let facts = command_facts(&events);
        assert_eq!(facts.len(), 5);
        assert_eq!(facts[0].program, "cargo");
        assert!(facts[2].failed, "exit marker 认失败");
        let repeated = repeated_commands(&events);
        assert_eq!(repeated.len(), 2, "{repeated:?}");
        assert_eq!(repeated[0].command, "cargo test -q");
        assert_eq!(repeated[0].count, 2, "夹读文件不打断前两次相同结果");
        assert_eq!(repeated[1].count, 2, "中间失败的 bash 重置后又一次同结果");
    }
}
