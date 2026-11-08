//! 屏蔽规则：通配、三种名字、规范化与出站拒绝（照 Magpie `TestDisabledGlob`）。

use crate::disabled::{admit, glob_match, match_disabled, names_of, normalize, refusal};
use crate::settings;

/// 逐条照抄 Magpie `TestDisabledGlob` 的通配用例（来源 `internal/provider/disabled_test.go`）。
#[test]
fn 通配规则照抄来源的用例() {
    for (pattern, value, expected) in [
        ("*fable*", "claude-fable-5", true),
        ("*fable*", "anthropic/claude-fable-5", true),
        ("claude-fable-5", "claude-fable-5", true),
        ("claude-fable-5", "claude-fable-5-mini", false),
        ("claude-fable-?", "claude-fable-5", true),
        ("anthropic/*", "anthropic/claude-fable-5", true),
        ("anthropic/*", "openrouter/anthropic/claude-fable-5", false),
        ("*", "", true),
        ("a*b*c", "aXbYc", true),
        ("a*b*c", "aXbY", false),
    ] {
        assert_eq!(
            glob_match(pattern, value),
            expected,
            "{pattern} ~ {value} 应 {}",
            if expected { "匹配" } else { "不匹配" }
        );
    }
}

#[test]
fn 一个模型认三种名字() {
    assert_eq!(
        names_of("openrouter", "anthropic/claude-fable-5"),
        vec![
            "anthropic/claude-fable-5",
            "openrouter/anthropic/claude-fable-5",
            "claude-fable-5",
        ]
    );
    assert_eq!(names_of("", " deepseek-flash "), vec!["deepseek-flash"]);
}

#[test]
fn 规则匹配三种名字且不分大小写() {
    // 首尾空白跳过；命中时原样返回命中的规则（不返回去掉空白的版本）。
    assert_eq!(
        match_disabled(
            &[" ".to_owned(), "*FABLE*".to_owned()],
            "openrouter",
            "anthropic/claude-fable-5"
        ),
        Some("*FABLE*".to_owned())
    );
    // 精确 id 认裸模型、连接/模型、以及 id 的最后一段。
    for pattern in [
        "claude-fable-5",
        "openrouter/anthropic/claude-fable-5",
        "anthropic/claude-fable-5",
    ] {
        assert_eq!(
            match_disabled(
                &[pattern.to_owned()],
                "openrouter",
                "anthropic/claude-fable-5"
            ),
            Some(pattern.to_owned()),
            "{pattern} 应命中"
        );
    }
    // 别的连接的模型不命中。
    assert_eq!(
        match_disabled(&["anthropic/*".to_owned()], "openrouter", "x"),
        None
    );
    // 不分大小写。
    assert_eq!(
        match_disabled(
            &["DEEPSEEK/DEEPSEEK-FLASH".to_owned()],
            "deepseek",
            "deepseek-flash"
        ),
        Some("DEEPSEEK/DEEPSEEK-FLASH".to_owned())
    );
    assert_eq!(
        match_disabled(&["m9".to_owned()], "deepseek", "deepseek-flash"),
        None
    );
}

#[test]
fn 规则表规范化保持顺序() {
    assert_eq!(
        normalize(&[
            " *FABLE* ".to_owned(),
            "".to_owned(),
            "m9".to_owned(),
            "*fable*".to_owned(),
            "   ".to_owned(),
            "M9 ".to_owned(),
        ]),
        vec!["*FABLE*".to_owned(), "m9".to_owned()],
        "去首尾空白、丢空规则、不分大小写去重、保持给出的顺序"
    );
}

#[test]
fn 屏蔽的模型过admit被拒且说明带规则() {
    let dir = tempfile::tempdir().expect("dir");
    let mut settings = settings::load(dir.path());
    settings.disabled_models = vec!["deepseek/*".to_owned()];
    settings::save(dir.path(), &settings).expect("save");

    let error = admit(dir.path(), "deepseek", "deepseek-flash").expect_err("被屏蔽");
    assert_eq!(error.code, model::ErrorCode::ModelDisabled, "错误码");
    assert_eq!(
        error.message,
        refusal("deepseek-flash", "deepseek/*"),
        "拒绝说明：模型名 + 命中的规则 + 两个出口"
    );
    assert!(
        error.message.contains("设置 › 模型与连接"),
        "说明里要有去设置的入口：{}",
        error.message
    );
    assert!(
        admit(dir.path(), "minimax", "MiniMax-M3").is_ok(),
        "没命中的模型照常放行"
    );
    assert!(
        admit(dir.path(), "deepseek", "deepseek-v4-pro").is_err(),
        "通配规则命中同连接的其它模型"
    );
}
