use super::thin_policy;

fn assert_missing_tool_rule(policy: &str) {
    for needle in [
        "names a specific tool, command, or approach",
        "report blocked on the first step",
        "do not substitute another tool",
        "do not simulate its output",
        "do not wait until the ready report",
    ] {
        assert!(
            policy.contains(needle),
            "thin_policy 缺关键句「{needle}」：{policy}"
        );
    }
}

#[test]
fn bash_off_policy_makes_a_missing_tool_a_first_step_blocked() {
    let policy = thin_policy(false);
    assert!(
        policy.contains("You cannot run shell commands"),
        "bash 关着的事实还在"
    );
    assert_missing_tool_rule(&policy);
}

#[test]
fn bash_on_policy_states_the_same_rule_for_other_missing_tools() {
    assert_missing_tool_rule(&thin_policy(true));
}
