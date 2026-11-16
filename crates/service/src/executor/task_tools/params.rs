//! 任务工具共用的参数解析。

use super::super::*;

pub(crate) fn parse_constraints(
    args: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, ToolFailure> {
    match args.get("constraints") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut constraints = Vec::with_capacity(items.len());
            for item in items {
                let Some(text) = item.as_str() else {
                    return Err(ToolFailure::plain("constraints 里的项必须是字符串"));
                };
                let text = text.trim();
                if text.is_empty() {
                    return Err(ToolFailure::plain("constraints 不能含空字符串"));
                }
                constraints.push(text.to_owned());
            }
            Ok(constraints)
        }
        Some(_) => Err(ToolFailure::plain("constraints 必须是字符串数组")),
    }
}

pub(crate) fn parse_object(arguments: &str) -> Result<serde_json::Map<String, Value>, ToolFailure> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| ToolFailure::plain("arguments must be a JSON object"))
}

pub(crate) fn string_arg(args: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}
