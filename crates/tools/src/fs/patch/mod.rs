//! Codex 风格补丁的解析与文本应用。
//!
//! 语法和匹配语义照 DSH 的 packages/fs/fs-patch（parse.ts / apply.ts，源自 Codex
//! rust-v0.158.0），按机制重写，不抄实现代码。

/// 一个连续 hunk：旧行、新行和可选锚文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hunk {
    /// 补丁输入里的一基行号（标记行或第一行隐式 hunk 行）。
    pub line: usize,
    /// @@ 之后的锚文本。
    pub context: Option<String>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub is_end_of_file: bool,
}

/// 一个有序文件操作；move 是带目的地的 update。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatchOp {
    Add {
        path: String,
        line: usize,
        content: String,
    },
    Delete {
        path: String,
        line: usize,
    },
    Update {
        path: String,
        line: usize,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
}

/// 解析失败：保留原始输入里的一基行号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchParseError {
    pub line: usize,
    pub reason: String,
}

impl PatchParseError {
    pub(crate) fn message(&self) -> String {
        format!("Invalid patch at line {}: {}", self.line, self.reason)
    }
}

/// 应用失败：第一个对不上的 hunk。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HunkFailure {
    pub hunk_line: usize,
    pub stage: &'static str,
    pub pattern: Vec<String>,
}

const BEGIN: &str = "*** Begin Patch";
const END: &str = "*** End Patch";
const EOF_MARKER: &str = "*** End of File";
const MOVE: &str = "*** Move to: ";
const ADD: &str = "*** Add File: ";
const DELETE: &str = "*** Delete File: ";
const UPDATE: &str = "*** Update File: ";

#[derive(Clone)]
struct InputLine {
    text: String,
    number: usize,
}

fn patch_lines(patch: &str) -> Result<Vec<InputLine>, PatchParseError> {
    let leading = patch.len() - patch.trim_start().len();
    let offset = patch[..leading].matches('\n').count();
    let trimmed = patch.trim();
    let mut lines: Vec<InputLine> = trimmed
        .split('\n')
        .enumerate()
        .map(|(index, raw)| InputLine {
            text: raw.strip_suffix('\r').unwrap_or(raw).to_owned(),
            number: offset + index + 1,
        })
        .collect();
    if lines.len() >= 4 {
        let first = lines.first().map(|line| line.text.as_str()).unwrap_or("");
        let last = lines.last().map(|line| line.text.as_str()).unwrap_or("");
        if matches!(first, "<<EOF" | "<<'EOF'" | "<<\"EOF\"") && last.ends_with("EOF") {
            lines = lines[1..lines.len() - 1].to_vec();
        }
    }
    let Some(start) = lines.first() else {
        return Err(PatchParseError {
            line: 1,
            reason: format!("Expected '{BEGIN}'"),
        });
    };
    if trim(&start.text) != BEGIN {
        return Err(PatchParseError {
            line: start.number,
            reason: format!("Expected '{BEGIN}'"),
        });
    }
    let Some(end) = lines.last() else {
        return Err(PatchParseError {
            line: 1,
            reason: format!("Expected '{END}'"),
        });
    };
    if trim(&end.text) != END {
        return Err(PatchParseError {
            line: end.number,
            reason: format!("Expected '{END}'"),
        });
    }
    Ok(lines)
}

/// 解析一个完整 Codex 风格补丁；空补丁返回空操作列表。
pub(crate) fn parse(patch: &str) -> Result<Vec<PatchOp>, PatchParseError> {
    let lines = patch_lines(patch)?;
    let last_index = lines.len() - 1;
    let mut operations: Vec<PatchOp> = Vec::new();
    let mut ended = false;
    for (index, line) in lines.iter().enumerate().skip(1) {
        let header = match operations.last() {
            Some(PatchOp::Update { .. }) if index != last_index => trim_end(&line.text),
            _ => trim(&line.text),
        };
        if ended && index != last_index {
            if !trim(&line.text).is_empty() {
                return Err(PatchParseError {
                    line: line.number,
                    reason: format!("Unexpected content after '{END}'"),
                });
            }
            continue;
        }
        if header == END {
            ensure_update(operations.last(), line.number)?;
            ended = true;
            continue;
        }
        if let Some(path) = header.strip_prefix(ADD) {
            ensure_update(operations.last(), line.number)?;
            operations.push(PatchOp::Add {
                path: path.to_owned(),
                line: line.number,
                content: String::new(),
            });
            continue;
        }
        if let Some(path) = header.strip_prefix(DELETE) {
            ensure_update(operations.last(), line.number)?;
            operations.push(PatchOp::Delete {
                path: path.to_owned(),
                line: line.number,
            });
            continue;
        }
        if let Some(path) = header.strip_prefix(UPDATE) {
            ensure_update(operations.last(), line.number)?;
            operations.push(PatchOp::Update {
                path: path.to_owned(),
                line: line.number,
                move_to: None,
                hunks: Vec::new(),
            });
            continue;
        }
        if operations.is_empty() && header.starts_with("*** Environment ID:") {
            return Err(PatchParseError {
                line: line.number,
                reason: "Environment ID headers are unsupported".to_owned(),
            });
        }
        if matches!(operations.last(), Some(PatchOp::Add { .. })) && line.text.starts_with('+') {
            if let Some(PatchOp::Add { content, .. }) = operations.last_mut() {
                content.push_str(&line.text[1..]);
                content.push('\n');
            }
            continue;
        }
        if matches!(operations.last(), Some(PatchOp::Update { .. })) {
            if let Some(PatchOp::Update { move_to, hunks, .. }) = operations.last_mut() {
                read_update(hunks, move_to, line)?;
            }
            continue;
        }
        return Err(PatchParseError {
            line: line.number,
            reason: "Expected an Add File, Delete File, or Update File header; added content must start with +".to_owned(),
        });
    }
    Ok(operations)
}

fn empty_hunk(hunk: &Hunk) -> bool {
    hunk.old_lines.is_empty() && hunk.new_lines.is_empty()
}

fn ensure_update(operation: Option<&PatchOp>, line: usize) -> Result<(), PatchParseError> {
    let Some(PatchOp::Update {
        path,
        line: op_line,
        hunks,
        ..
    }) = operation
    else {
        return Ok(());
    };
    let Some(last) = hunks.last() else {
        return Err(PatchParseError {
            line: *op_line,
            reason: format!("Update for '{path}' requires at least one hunk"),
        });
    };
    if empty_hunk(last) {
        return Err(PatchParseError {
            line,
            reason: "Update hunk must contain context, added, or removed lines".to_owned(),
        });
    }
    Ok(())
}

fn read_update(
    hunks: &mut Vec<Hunk>,
    move_to: &mut Option<String>,
    line: &InputLine,
) -> Result<(), PatchParseError> {
    let text = trim_end(&line.text);
    let previous_eof = hunks
        .last()
        .map(|hunk| hunk.is_end_of_file)
        .unwrap_or(false);
    let marker = text == "@@" || text.starts_with("@@ ");
    if previous_eof {
        if text.is_empty() {
            return Ok(());
        }
        if !marker {
            return Err(PatchParseError {
                line: line.number,
                reason: "Expected '@@' after '*** End of File'".to_owned(),
            });
        }
    }
    if hunks.is_empty() && move_to.is_none() && text.starts_with(MOVE) {
        *move_to = Some(text[MOVE.len()..].to_owned());
        return Ok(());
    }
    if marker {
        if let Some(previous) = hunks.last()
            && empty_hunk(previous)
        {
            return Err(PatchParseError {
                line: line.number,
                reason: "Update hunk must contain context, added, or removed lines".to_owned(),
            });
        }
        let context = text.strip_prefix("@@ ").map(str::to_owned);
        hunks.push(Hunk {
            line: line.number,
            context,
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            is_end_of_file: false,
        });
        return Ok(());
    }
    if text == EOF_MARKER {
        if let Some(previous) = hunks.last_mut() {
            if empty_hunk(previous) {
                return Err(PatchParseError {
                    line: line.number,
                    reason: "Update hunk must contain context, added, or removed lines".to_owned(),
                });
            }
            previous.is_end_of_file = true;
        }
        return Ok(());
    }
    let prefix = line.text.chars().next();
    if !line.text.is_empty() && !matches!(prefix, Some(' ') | Some('+') | Some('-')) {
        return Err(PatchParseError {
            line: line.number,
            reason: "Expected a context (' '), added ('+'), removed ('-') line, or '@@'".to_owned(),
        });
    }
    if hunks.is_empty() {
        hunks.push(Hunk {
            line: line.number,
            context: None,
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            is_end_of_file: false,
        });
    }
    let content = if line.text.is_empty() {
        String::new()
    } else {
        line.text[1..].to_owned()
    };
    let hunk = hunks.last_mut().expect("hunk 已在上一步创建");
    match prefix {
        Some('+') => hunk.new_lines.push(content),
        Some('-') => hunk.old_lines.push(content),
        _ => {
            hunk.old_lines.push(content.clone());
            hunk.new_lines.push(content);
        }
    }
    Ok(())
}

fn trim(value: &str) -> &str {
    value.trim()
}

fn trim_end(value: &str) -> &str {
    value.trim_end()
}

type Normalize = fn(&str) -> String;

const MATCH_PASSES: [Normalize; 4] = [
    pass_identity,
    pass_trim_end,
    pass_trim,
    normalize_punctuation,
];

fn pass_identity(value: &str) -> String {
    value.to_owned()
}

fn pass_trim_end(value: &str) -> String {
    trim_whitespace(value, false)
}

fn pass_trim(value: &str) -> String {
    trim_whitespace(value, true)
}

fn trim_whitespace(value: &str, leading: bool) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut start = 0;
    let mut end = chars.len();
    if leading {
        while start < end && chars[start].is_whitespace() {
            start += 1;
        }
    }
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    chars[start..end].iter().collect()
}

fn normalize_punctuation(value: &str) -> String {
    trim_whitespace(value, true)
        .chars()
        .map(|character| match character {
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{2018}'..='\u{201B}' => '\'',
            '\u{201C}'..='\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let last = lines.len() - pattern.len();
    let first = if eof { last } else { start };
    for normalize in MATCH_PASSES {
        let expected: Vec<String> = pattern.iter().map(|line| normalize(line)).collect();
        for index in first..=last {
            if expected
                .iter()
                .enumerate()
                .all(|(offset, want)| normalize(&lines[index + offset]) == *want)
            {
                return Some(index);
            }
        }
    }
    None
}

struct Replacement {
    start: usize,
    old_length: usize,
    new_lines: Vec<String>,
}

/// 把解析好的 hunk 应用到 LF 归一文本；失败返回第一个对不上的 hunk。
pub(crate) fn apply_hunks(before: &str, hunks: &[Hunk]) -> Result<String, HunkFailure> {
    let mut original: Vec<String> = before.split('\n').map(str::to_owned).collect();
    if original.last().map(String::as_str) == Some("") {
        original.pop();
    }
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut line_index = 0usize;
    for hunk in hunks {
        if let Some(context) = &hunk.context {
            let pattern = vec![context.clone()];
            let Some(found) = seek_sequence(&original, &pattern, line_index, false) else {
                return Err(HunkFailure {
                    hunk_line: hunk.line,
                    stage: "context",
                    pattern,
                });
            };
            line_index = found + 1;
        }
        if hunk.old_lines.is_empty() {
            let start = if original.last().map(String::as_str) == Some("") {
                original.len() - 1
            } else {
                original.len()
            };
            replacements.push(Replacement {
                start,
                old_length: 0,
                new_lines: hunk.new_lines.clone(),
            });
            continue;
        }
        let mut pattern = hunk.old_lines.clone();
        let mut new_lines = hunk.new_lines.clone();
        let mut found = seek_sequence(&original, &pattern, line_index, hunk.is_end_of_file);
        if found.is_none() && pattern.last().map(String::as_str) == Some("") {
            pattern.pop();
            if new_lines.last().map(String::as_str) == Some("") {
                new_lines.pop();
            }
            found = seek_sequence(&original, &pattern, line_index, hunk.is_end_of_file);
        }
        let Some(found) = found else {
            return Err(HunkFailure {
                hunk_line: hunk.line,
                stage: "oldLines",
                pattern: hunk.old_lines.clone(),
            });
        };
        line_index = found + pattern.len();
        replacements.push(Replacement {
            start: found,
            old_length: pattern.len(),
            new_lines,
        });
    }
    replacements.sort_by_key(|replacement| replacement.start);
    let mut lines = original;
    for replacement in replacements.iter().rev() {
        let mut next: Vec<String> = lines[..replacement.start].to_vec();
        next.extend(replacement.new_lines.iter().cloned());
        next.extend(
            lines[replacement.start + replacement.old_length..]
                .iter()
                .cloned(),
        );
        lines = next;
    }
    if lines.last().map(String::as_str) != Some("") {
        lines.push(String::new());
    }
    Ok(lines.join("\n"))
}
