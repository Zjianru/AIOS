//! 建约定的 details：确认卡校验和运行时建约定共用这一份解析器。
//!
//! 模型可能给 startAt 数字、数字字符串、RFC3339、本地「YYYY-MM-DD HH:MM:SS」，或者用
//! delaySeconds 代替；binding 可能是字符串，也可能是 {"kind":"chat"/"kind"/"task", "taskId":...}。
//! 这里把它们规范化成同一份 canonical JSON（startAt 绝对毫秒、binding 规范对象），
//! 确认卡事实和真正建约定都从这份 canonical JSON 读，避免两边各认一套。

use chrono::{DateTime, Local, LocalResult, NaiveDateTime, TimeZone};
use serde_json::{Value, json};

use calendar::{Binding, CycleSpec, NewAppointment, Schedule, WatchSource};

/// now 是解析 delaySeconds 和本地时间要用的毫秒时钟；生产用系统时钟，测试可注入。
pub(crate) fn canonicalize(details: &Value, now: i64) -> Result<Value, String> {
    let object = details
        .as_object()
        .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
    let text = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let title = text("title").ok_or_else(|| "details.title 必须是非空字符串".to_owned())?;
    let condition =
        text("condition").ok_or_else(|| "details.condition 必须是非空字符串".to_owned())?;
    let rule = text("rule").ok_or_else(|| "details.rule 必须是非空字符串".to_owned())?;
    let timezone = text("timezone").unwrap_or_else(|| "Asia/Shanghai".to_owned());
    let start_at = parse_start_at(object.get("startAt"), object.get("delaySeconds"), now)?;
    let binding = parse_binding(object.get("binding"), object.get("taskId"))?;
    let watch = parse_watch(object.get("watch"))?;
    let cycle = parse_cycle(object)?;

    Ok(json!({
        "title": title,
        "condition": condition,
        "rule": rule,
        "timezone": timezone,
        "startAt": start_at,
        "binding": binding_json(&binding),
        "watch": watch.as_ref().map(watch_json).unwrap_or(Value::Null),
        "cycleTaskId": cycle.as_ref().map(|cycle| cycle.task_id.clone()),
        "maxRounds": cycle.as_ref().and_then(|cycle| cycle.max_rounds),
    }))
}

/// 从 canonical JSON（也直接接受原始 details）造 NewAppointment；运行时建约定只走这里。
pub(crate) fn new_appointment(details: &Value) -> Result<NewAppointment, String> {
    let canonical = if details.get("startAt").and_then(Value::as_i64).is_some() {
        details.clone()
    } else {
        canonicalize(details, now_ms())?
    };
    let object = canonical
        .as_object()
        .ok_or_else(|| "details 必须是 JSON 对象".to_owned())?;
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("canonical details 缺 {name}"))
    };
    let start_at = object
        .get("startAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| "canonical details.startAt 必须是毫秒时间戳".to_owned())?;
    let binding = parse_binding(object.get("binding"), object.get("taskId"))?;
    let watch = parse_watch(object.get("watch"))?;
    let cycle = parse_cycle(object)?;
    Ok(NewAppointment {
        title: field("title")?,
        condition: field("condition")?,
        schedule: Schedule {
            rule: field("rule")?,
            timezone: field("timezone")?,
            start_at,
        },
        binding,
        watch,
        cycle,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// 单独解析一个 startAt 值（改约定用），规则和建约定完全一致。
pub(crate) fn parse_start_at_value(value: &Value) -> Result<i64, String> {
    parse_start_at(Some(value), None, now_ms())
}

/// startAt：i64 / 数字字符串 / RFC3339 / 本地日期时间；没有 startAt 时接受 delaySeconds。
fn parse_start_at(
    start_at: Option<&Value>,
    delay: Option<&Value>,
    now: i64,
) -> Result<i64, String> {
    if let Some(value) = start_at {
        return match value {
            Value::Number(_) => value
                .as_i64()
                .ok_or_else(|| "details.startAt 超出范围".to_owned()),
            Value::String(raw) => parse_start_at_text(raw.trim()),
            _ => Err("details.startAt 必须是毫秒时间戳或时间字符串".to_owned()),
        };
    }
    let Some(value) = delay else {
        return Err("details.startAt 或 delaySeconds 必须给一个".to_owned());
    };
    let seconds = match value {
        Value::Number(_) => value
            .as_i64()
            .ok_or_else(|| "details.delaySeconds 超出范围".to_owned())?,
        Value::String(raw) => raw
            .trim()
            .parse::<i64>()
            .map_err(|_| "details.delaySeconds 必须是整数秒数".to_owned())?,
        _ => return Err("details.delaySeconds 必须是整数秒数".to_owned()),
    };
    if seconds < 0 {
        return Err("details.delaySeconds 不能是负数".to_owned());
    }
    now.checked_add(seconds.saturating_mul(1000))
        .ok_or_else(|| "details.delaySeconds 太大".to_owned())
}

fn parse_start_at_text(raw: &str) -> Result<i64, String> {
    if let Ok(millis) = raw.parse::<i64>() {
        return Ok(millis);
    }
    if let Ok(time) = DateTime::parse_from_rfc3339(raw) {
        return Ok(time.timestamp_millis());
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, format) {
            return local_millis(naive);
        }
    }
    Err(format!(
        "details.startAt 认不出这个时间：{raw:?}；用毫秒时间戳、数字字符串、RFC3339、YYYY-MM-DD HH:MM:SS，或改用 delaySeconds"
    ))
}

fn local_millis(naive: NaiveDateTime) -> Result<i64, String> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(time) => Ok(time.timestamp_millis()),
        LocalResult::Ambiguous(earliest, _) => Ok(earliest.timestamp_millis()),
        LocalResult::None => Err(format!(
            "本机时区里没有这个本地时间：{}",
            naive.format("%Y-%m-%d %H:%M:%S")
        )),
    }
}

/// binding：字符串 kind/chat/task；或对象 {"kind":"chat"/"kind"/"task","taskId":...}。
fn parse_binding(value: Option<&Value>, fallback_task: Option<&Value>) -> Result<Binding, String> {
    let Some(value) = value else {
        return Ok(Binding::Kind);
    };
    let (kind, task_id) = match value {
        Value::String(kind) => (kind.trim(), fallback_task.and_then(Value::as_str)),
        Value::Object(object) => (
            object
                .get("kind")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or(""),
            object
                .get("taskId")
                .and_then(Value::as_str)
                .or_else(|| object.get("task_id").and_then(Value::as_str)),
        ),
        _ => return Err("details.binding 必须是字符串或对象".to_owned()),
    };
    match kind {
        "" | "kind" | "chat" => Ok(Binding::Kind),
        "task" => task_id
            .filter(|id| !id.is_empty())
            .map(|id| Binding::Task(id.to_owned()))
            .ok_or_else(|| "绑定任务时 details.binding.taskId 或 details.taskId 必须给".to_owned()),
        other => Err(format!(
            "details.binding.kind 只认 chat/kind/task，收到 {other:?}"
        )),
    }
}

fn parse_watch(value: Option<&Value>) -> Result<Option<WatchSource>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or_else(|| "details.watch 必须是对象".to_owned())?;
    let kind = object.get("kind").and_then(Value::as_str).unwrap_or("");
    let target = object
        .get("target")
        .and_then(Value::as_str)
        .filter(|target| !target.is_empty())
        .ok_or_else(|| "details.watch.target 必须是非空字符串".to_owned())?;
    match kind {
        "url" => Ok(Some(WatchSource::Url(target.to_owned()))),
        "file" => Ok(Some(WatchSource::File(target.to_owned()))),
        "taskFacts" => Ok(Some(WatchSource::TaskFacts(target.to_owned()))),
        other => Err(format!(
            "details.watch.kind 只认 url/file/taskFacts，收到 {other:?}"
        )),
    }
}

fn parse_cycle(object: &serde_json::Map<String, Value>) -> Result<Option<CycleSpec>, String> {
    let Some(task_id) = object
        .get("cycleTaskId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let max_rounds = match object.get("maxRounds") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let number = match value {
                Value::Number(_) => value.as_u64(),
                Value::String(raw) => raw.trim().parse::<u64>().ok(),
                _ => None,
            }
            .ok_or_else(|| "details.maxRounds 必须是非负整数".to_owned())?;
            Some(u32::try_from(number).map_err(|_| "details.maxRounds 太大".to_owned())?)
        }
    };
    Ok(Some(CycleSpec {
        task_id: task_id.to_owned(),
        max_rounds,
    }))
}

fn binding_json(binding: &Binding) -> Value {
    match binding {
        Binding::Kind => json!({ "kind": "kind" }),
        Binding::Task(task_id) => json!({ "kind": "task", "taskId": task_id }),
    }
}

fn watch_json(watch: &WatchSource) -> Value {
    match watch {
        WatchSource::Url(url) => json!({ "kind": "url", "target": url }),
        WatchSource::File(file) => json!({ "kind": "file", "target": file }),
        WatchSource::TaskFacts(task) => json!({ "kind": "taskFacts", "target": task }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_935_800_000;

    fn base() -> serde_json::Map<String, Value> {
        let mut object = serde_json::Map::new();
        object.insert("title".to_owned(), json!("提醒"));
        object.insert("condition".to_owned(), json!("一分钟后提醒我喝水"));
        object.insert("rule".to_owned(), json!("FREQ=DAILY"));
        object.insert("timezone".to_owned(), json!("Asia/Shanghai"));
        object
    }

    #[test]
    fn start_at_accepts_number_number_string_and_datetime_strings() {
        let mut number = base();
        number.insert("startAt".to_owned(), json!(NOW));
        assert_eq!(
            canonicalize(&Value::Object(number), 0).unwrap()["startAt"],
            json!(NOW)
        );

        let mut number_string = base();
        number_string.insert("startAt".to_owned(), json!(NOW.to_string()));
        assert_eq!(
            canonicalize(&Value::Object(number_string), 0).unwrap()["startAt"],
            json!(NOW)
        );

        let mut rfc = base();
        rfc.insert("startAt".to_owned(), json!("2026-11-17T09:00:00+08:00"));
        assert_eq!(
            canonicalize(&Value::Object(rfc), 0).unwrap()["startAt"],
            json!(1_794_877_200_000_i64)
        );

        let mut local = base();
        local.insert("startAt".to_owned(), json!("2026-11-17 09:00:00"));
        let parsed = canonicalize(&Value::Object(local), 0).unwrap()["startAt"]
            .as_i64()
            .expect("本地时间");
        let expected = crate::appointments::local_millis(
            NaiveDateTime::parse_from_str("2026-11-17 09:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
        )
        .unwrap();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn delay_seconds_and_binding_objects_are_normalized() {
        let mut delay = base();
        delay.insert("delaySeconds".to_owned(), json!("60"));
        delay.insert("binding".to_owned(), json!({ "kind": "chat" }));
        let canonical = canonicalize(&Value::Object(delay), NOW).unwrap();
        assert_eq!(canonical["startAt"], json!(NOW + 60_000));
        assert_eq!(canonical["binding"]["kind"], "kind");

        let mut task = base();
        task.insert("delaySeconds".to_owned(), json!(5));
        task.insert(
            "binding".to_owned(),
            json!({ "kind": "task", "taskId": "task-demo" }),
        );
        let canonical = canonicalize(&Value::Object(task), NOW).unwrap();
        assert_eq!(canonical["binding"]["kind"], "task");
        assert_eq!(canonical["binding"]["taskId"], "task-demo");
    }

    #[test]
    fn unknown_binding_kind_is_refused_instead_of_silently_kind() {
        let mut bad = base();
        bad.insert("startAt".to_owned(), json!(NOW));
        bad.insert("binding".to_owned(), json!({ "kind": "nope" }));
        let error = canonicalize(&Value::Object(bad), NOW).unwrap_err();
        assert!(error.contains("只认 chat/kind/task"), "{error}");
    }
}
