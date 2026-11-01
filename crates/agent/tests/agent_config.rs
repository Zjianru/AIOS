//! T005.0 接口地基：AgentConfig 字段、默认值与非法值校验。

mod common;

use std::path::PathBuf;

use agent::{Agent, AgentConfig, AgentError, Deps};
use common::model_config;
use model::DeepSeek;
use session::SessionStore;
use tools::ToolSet;

fn home_agents() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agents")
}

async fn create_result(config: AgentConfig) -> Result<Agent, AgentError> {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    Agent::create(
        config,
        Deps {
            model: DeepSeek::new(model_config("http://127.0.0.1:1")).unwrap(),
            tools: ToolSet::empty(),
            store,
        },
    )
    .await
}

#[test]
fn defaults_match_dsh() {
    let config = AgentConfig::default();
    assert_eq!(config.max_parallel_tool_calls, 10);
    assert!((config.compaction_threshold_ratio - 0.8).abs() < f64::EPSILON);
    assert!((config.compaction_retain_ratio - 0.16).abs() < f64::EPSILON);
    assert_eq!(config.compaction_headroom_tokens, 65_536);
    assert_eq!(config.spill_max_inline_tokens, 12_500);
    assert_eq!(config.instruction_roots, vec![home_agents()]);
    assert_eq!(config.skill_roots, vec![home_agents().join("skills")]);
}

#[tokio::test]
async fn rejects_invalid_values() {
    let mut config = AgentConfig {
        max_parallel_tool_calls: 0,
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config.clone()).await,
        Err(AgentError::InvalidConfig {
            field: "max_parallel_tool_calls",
            ..
        })
    ));

    config = AgentConfig {
        compaction_threshold_ratio: 0.0,
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config.clone()).await,
        Err(AgentError::InvalidConfig {
            field: "compaction_threshold_ratio",
            ..
        })
    ));

    config = AgentConfig {
        compaction_threshold_ratio: 1.5,
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config.clone()).await,
        Err(AgentError::InvalidConfig {
            field: "compaction_threshold_ratio",
            ..
        })
    ));

    config = AgentConfig {
        compaction_retain_ratio: 0.9,
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config.clone()).await,
        Err(AgentError::InvalidConfig {
            field: "compaction_retain_ratio",
            ..
        })
    ));

    config = AgentConfig {
        compaction_headroom_tokens: 0,
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config.clone()).await,
        Err(AgentError::InvalidConfig {
            field: "compaction_headroom_tokens",
            ..
        })
    ));

    config = AgentConfig {
        instruction_roots: vec![PathBuf::from("relative/agents")],
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config.clone()).await,
        Err(AgentError::InvalidConfig {
            field: "instruction_roots",
            ..
        })
    ));

    config = AgentConfig {
        skill_roots: vec![PathBuf::from("relative/skills")],
        ..AgentConfig::default()
    };
    assert!(matches!(
        create_result(config).await,
        Err(AgentError::InvalidConfig {
            field: "skill_roots",
            ..
        })
    ));
}

#[tokio::test]
async fn accepts_valid_non_default_values() {
    let config = AgentConfig {
        max_parallel_tool_calls: 2,
        compaction_threshold_ratio: 0.5,
        compaction_retain_ratio: 0.1,
        compaction_headroom_tokens: 1024,
        compaction_summary_max_tokens: None,
        compaction_retries: 1,
        compaction_max_overflow_retries: 1,
        compaction_model_policies: Vec::new(),
        spill_max_inline_tokens: 100,
        instruction_roots: vec![PathBuf::from("/tmp/aios-agents")],
        skill_roots: vec![PathBuf::from("/tmp/aios-agents/skills")],
    };
    let agent = create_result(config).await.unwrap();
    agent.idle().await;
}
