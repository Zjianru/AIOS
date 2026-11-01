//! T006.5c1：消息 IR 的 id/source 与五角色 JSON 形状（照 DSH llm/llm/src/message.ts:139-180、:206-260）。

use serde_json::json;

use crate::{ContentBlock, Message, MessageId, MessageSource, Role};

/// uuid v4 形状：8-4-4-4-12 十六进制，版本位 4、变体位 8/9/a/b。
fn assert_uuid_v4(raw: &str) {
    let parts: Vec<&str> = raw.split('-').collect();
    assert_eq!(
        parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "id 应是 uuid v4 形状：{raw}"
    );
    assert!(
        raw.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-'),
        "id 只含十六进制和连字符：{raw}"
    );
    assert_eq!(parts[2].as_bytes()[0], b'4', "uuid 版本位应是 4：{raw}");
    assert!(
        matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
        "uuid 变体位应是 8/9/a/b：{raw}"
    );
}

#[test]
fn message_has_stable_unique_id() {
    let first = Message::user_text("第一条");
    let second = Message::user_text("第二条");
    // 新建时生成 uuid v4，且每条消息不同。
    assert_uuid_v4(first.id.as_str());
    assert_ne!(first.id, second.id, "每条消息的 id 唯一");

    // 序列化/反序列化后 id 不变（续接、日志回放靠这个）。
    let json = serde_json::to_string(&first).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, first.id, "反序列化后 id 不变");
    assert_eq!(parsed, first, "整条消息 round-trip 不变");

    // 派生（克隆）也保持同一个 id。
    assert_eq!(first.clone().id, first.id);

    // 显式恢复已有身份：日志/派生消息用 parse 保留原值。
    let restored = Message::with_source(
        MessageId::parse("11111111-1111-4111-8111-111111111111"),
        Role::User,
        vec![ContentBlock::text("恢复")],
        MessageSource::User,
    );
    assert_eq!(restored.id.as_str(), "11111111-1111-4111-8111-111111111111");
}

#[test]
fn source_kind_and_fields_match_dsh() {
    // 四个 source kind 的字段照 DSH MessageSourceMap。
    let user = Message::user_text("你好");
    assert_eq!(user.source, MessageSource::User);
    let system = Message::system_prompt("系统提示");
    assert_eq!(system.source, MessageSource::SystemPrompt);
    let assistant = Message::assistant(
        vec![ContentBlock::text("回答")],
        "deepseek",
        "deepseek-flash",
    );
    let MessageSource::Model {
        provider,
        model,
        replay_state,
    } = &assistant.source
    else {
        panic!("assistant 的 source 应是 model");
    };
    assert_eq!(provider, "deepseek");
    assert_eq!(model, "deepseek-flash");
    assert!(
        replay_state.is_none(),
        "AIOS 没有 adapter replayState，恒 None"
    );
    let result = Message::tool_result("call_0", "ok", false);
    assert_eq!(
        result.source,
        MessageSource::Tool {
            call_id: "call_0".to_owned()
        }
    );

    // JSON 里 source 是带 kind 的对象；model 的路由字段在 source 里。
    let assistant_json = serde_json::to_value(&assistant).unwrap();
    assert_eq!(assistant_json["source"]["kind"], "model");
    assert_eq!(assistant_json["source"]["provider"], "deepseek");
    assert_eq!(assistant_json["source"]["model"], "deepseek-flash");
    assert!(assistant_json["source"].get("replayState").is_none());
    let user_json = serde_json::to_value(&user).unwrap();
    assert_eq!(user_json["source"], json!({"kind": "user"}));
    let system_json = serde_json::to_value(&system).unwrap();
    assert_eq!(system_json["source"], json!({"kind": "system-prompt"}));
    let tool_json = serde_json::to_value(&result).unwrap();
    assert_eq!(
        tool_json["source"],
        json!({"kind": "tool", "callId": "call_0"})
    );
}

#[test]
fn five_role_json_shapes() {
    let system = Message::system_prompt("系统提示");
    let developer = Message::developer(vec![ContentBlock::text("tool-addition")]);
    let user = Message::user_text("你好");
    let assistant = Message::assistant(
        vec![ContentBlock::text("回答")],
        "deepseek",
        "deepseek-flash",
    );
    let tool = Message::tool_result("call_0", "结果文本", true);

    let cases: [(&Message, &str); 5] = [
        (&system, "system"),
        (&developer, "developer"),
        (&user, "user"),
        (&assistant, "assistant"),
        (&tool, "tool"),
    ];
    for (message, role) in cases {
        let value = serde_json::to_value(message).unwrap();
        assert_uuid_v4(value["id"].as_str().expect("消息 JSON 带 id"));
        assert_eq!(value["role"], role);
        assert!(value["content"].is_array(), "{role} 的 content 是块数组");
        assert!(
            value["source"]["kind"].is_string(),
            "{role} 的 source 带 kind：{value}"
        );
    }

    // system / developer / user / assistant 的 source 各自照 DSH。
    assert_eq!(
        serde_json::to_value(&system).unwrap()["source"],
        json!({"kind": "system-prompt"})
    );
    assert_eq!(
        serde_json::to_value(&developer).unwrap()["source"],
        json!({"kind": "user"})
    );
    assert_eq!(
        serde_json::to_value(&assistant).unwrap()["source"]["kind"],
        "model"
    );

    // tool 消息照 DSH：消息级 toolCallId/isError，content 是结果内容块（不再有 tool_result 块）。
    let tool_json = serde_json::to_value(&tool).unwrap();
    assert_eq!(tool_json["role"], "tool");
    assert_eq!(tool_json["toolCallId"], "call_0");
    assert_eq!(tool_json["isError"], true);
    assert_eq!(
        tool_json["content"],
        json!([{"type": "text", "text": "结果文本"}]),
        "tool 消息的 content 是结果内容块"
    );
    // 成功的工具结果不写 isError（DSH isError?: boolean）。
    let ok = Message::tool_result("call_0", "ok", false);
    assert!(serde_json::to_value(&ok).unwrap().get("isError").is_none());
}
