//! 日历库错误：存储、规则解析、状态迁移。

use std::fmt;

#[derive(Debug)]
pub enum CalendarError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    InvalidTimezone(String),
    InvalidRule(String),
    InvalidSchedule(String),
    AppointmentNotFound(String),
    StaleVersion { expected: u32, actual: u32 },
    RunNotFound(i64),
    InvalidStatus(String),
    InvalidRunResult { from: String, to: String },
    InvalidInput(String),
}

impl fmt::Display for CalendarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "日历库出错：{error}"),
            Self::Io(error) => write!(formatter, "日历库文件出错：{error}"),
            Self::InvalidTimezone(name) => write!(formatter, "未知时区：{name}"),
            Self::InvalidRule(rule) => write!(formatter, "规则读不出来：{rule}"),
            Self::InvalidSchedule(reason) => write!(formatter, "规约不对：{reason}"),
            Self::AppointmentNotFound(id) => write!(formatter, "约定不存在：{id}"),
            Self::StaleVersion { expected, actual } => {
                write!(
                    formatter,
                    "约定刚被改过（期望第 {expected} 版，实际第 {actual} 版）"
                )
            }
            Self::RunNotFound(id) => write!(formatter, "运行记录不存在：{id}"),
            Self::InvalidStatus(status) => write!(formatter, "未知状态：{status}"),
            Self::InvalidRunResult { from, to } => {
                write!(formatter, "运行结果不能从 {from} 走到 {to}")
            }
            Self::InvalidInput(reason) => write!(formatter, "输入不对：{reason}"),
        }
    }
}

impl std::error::Error for CalendarError {}

impl From<rusqlite::Error> for CalendarError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for CalendarError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
