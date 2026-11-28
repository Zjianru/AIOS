//! 规则数据与严格度冲突（T103.3 起步：数据/版本/纯函数；注入与页面待接）。

use crate::ServiceError;
use crate::root_lifecycle::RootHost;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 一条规则。scope 只有 global 和 project:<id>；角色没有规则（D145）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub rev: u64,
    pub scope: String,
    pub category: String,
    #[serde(default)]
    pub target: String,
    pub strictness: String,
    pub text: String,
    #[serde(default)]
    pub source: String,
}

/// rules.json：version 每次增删改 +1；rules 是当前全部规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RulesFile {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct RuleConflict {
    /// 被放宽而不生效的更近范围规则。
    pub rule_id: String,
    /// 压住它的全局规则。
    pub overridden_by: String,
    pub reason: String,
}

fn strictness_rank(value: &str) -> Option<u8> {
    match value {
        "deny" => Some(0),
        "ask" => Some(1),
        "allow" => Some(2),
        _ => None,
    }
}

/// 03 篇 5.7：越近的范围只能取更严或相同的严格度。
/// 返回项目规则里比同 category/target 的全局规则更松、因此不生效的项。
pub fn conflicts(rules: &[Rule]) -> Vec<RuleConflict> {
    let mut out = Vec::new();
    for project in rules
        .iter()
        .filter(|rule| rule.scope.starts_with("project:"))
    {
        let Some(project_rank) = strictness_rank(&project.strictness) else {
            continue;
        };
        for global in rules.iter().filter(|rule| {
            rule.scope == "global"
                && rule.category == project.category
                && rule.target == project.target
        }) {
            let Some(global_rank) = strictness_rank(&global.strictness) else {
                continue;
            };
            if project_rank > global_rank {
                out.push(RuleConflict {
                    rule_id: project.id.clone(),
                    overridden_by: global.id.clone(),
                    reason: "更近范围只能更严或相同；这条比全局更松，不生效".to_owned(),
                });
            }
        }
    }
    out
}

pub fn path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("rules.json")
}

pub fn load(data_dir: &Path) -> RulesFile {
    std::fs::read(path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(data_dir: &Path, file: &RulesFile) -> Result<(), String> {
    let mut json = serde_json::to_vec(file).map_err(|error| error.to_string())?;
    json.push(b'\n');
    credentials::atomic::write(
        &path(data_dir),
        &json,
        credentials::atomic::TargetKind::Shared,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, scope: &str, strictness: &str) -> Rule {
        Rule {
            id: id.to_owned(),
            rev: 1,
            scope: scope.to_owned(),
            category: "动作".to_owned(),
            target: "删除文件".to_owned(),
            strictness: strictness.to_owned(),
            text: id.to_owned(),
            source: "用户".to_owned(),
        }
    }

    #[test]
    fn closer_scope_cannot_be_looser_than_global() {
        let rules = vec![
            rule("g-deny", "global", "deny"),
            rule("p-allow", "project:p1", "allow"),
            rule("p-ask", "project:p1", "ask"),
        ];
        let found = conflicts(&rules);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|item| item.overridden_by == "g-deny"));
        assert_eq!(found[0].rule_id, "p-allow");
        assert_eq!(found[1].rule_id, "p-ask");
    }

    #[test]
    fn equal_or_stricter_project_rule_is_not_a_conflict() {
        let rules = vec![
            rule("g-ask", "global", "ask"),
            rule("p-ask", "project:p1", "ask"),
            rule("p-deny", "project:p1", "deny"),
        ];
        assert!(conflicts(&rules).is_empty());
    }

    #[test]
    fn version_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = RulesFile {
            version: 3,
            rules: vec![rule("g", "global", "ask")],
        };
        save(dir.path(), &file).unwrap();
        assert_eq!(load(dir.path()), file);
    }
}

impl RootHost {
    pub(crate) async fn rules_read(
        &self,
    ) -> Result<crate::protocol::RulesReadResponse, ServiceError> {
        let file = load(&self.options.data_dir);
        Ok(crate::protocol::RulesReadResponse {
            version: file.version,
            conflicts: conflicts(&file.rules),
            rules: file.rules,
        })
    }

    fn rules_write(
        &self,
        change: impl FnOnce(&mut RulesFile),
    ) -> Result<crate::protocol::RulesReadResponse, ServiceError> {
        let mut file = load(&self.options.data_dir);
        change(&mut file);
        file.version = file.version.saturating_add(1);
        validate(&file.rules)?;
        save(&self.options.data_dir, &file).map_err(ServiceError::SettingsInvalid)?;
        Ok(crate::protocol::RulesReadResponse {
            version: file.version,
            conflicts: conflicts(&file.rules),
            rules: file.rules,
        })
    }

    pub(crate) async fn rule_save(
        &self,
        request: crate::protocol::RuleSaveRequest,
    ) -> Result<crate::protocol::RulesReadResponse, ServiceError> {
        let response = self.rules_write(|file| {
            let mut rule = request.rule.clone();
            rule.rev = file.version.saturating_add(1);
            if rule.source.trim().is_empty() {
                rule.source = "用户".to_owned();
            }
            if let Some(existing) = file.rules.iter_mut().find(|item| item.id == rule.id) {
                *existing = rule;
            } else {
                file.rules.push(rule);
            }
        })?;
        self.emit_settings_changed();
        Ok(response)
    }

    pub(crate) async fn rule_delete(
        &self,
        request: crate::protocol::RulesDeleteRequest,
    ) -> Result<crate::protocol::RulesReadResponse, ServiceError> {
        let response = self.rules_write(|file| file.rules.retain(|rule| rule.id != request.id))?;
        self.emit_settings_changed();
        Ok(response)
    }
}

fn validate(rules: &[Rule]) -> Result<(), ServiceError> {
    for rule in rules {
        if rule.id.trim().is_empty()
            || rule.category.trim().is_empty()
            || rule.text.trim().is_empty()
        {
            return Err(ServiceError::SettingsInvalid(
                "规则 id/类别/内容不能为空".to_owned(),
            ));
        }
        let scope_ok = rule.scope == "global"
            || rule
                .scope
                .strip_prefix("project:")
                .is_some_and(|id| !id.trim().is_empty());
        if !scope_ok {
            return Err(ServiceError::SettingsInvalid(
                "规则范围只能是 global 或 project:<id>".to_owned(),
            ));
        }
        if strictness_rank(&rule.strictness).is_none() {
            return Err(ServiceError::SettingsInvalid(
                "规则严格度只能是 deny/ask/allow".to_owned(),
            ));
        }
    }
    Ok(())
}
