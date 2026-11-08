//! T019.E：/api 的方法集合必须是 service::Call 的子集。
//!
//! 页面发 /api 的方法清单只有一处字面量：`web/src/data/service.ts` 的 `method: '...'`。
//! 这里在编译期把那份源码带进来，逐条确认 serde 认识（不是 Call 的 unknown variant）。
//! monitor-report 以前在 web 解码前被字符串截走；进 Call 以后这个例外不再需要。

use service::Call;

/// 页面的 /api 调用面（编译期读入，文件挪了会编译失败，不会静默跳过）。
const SERVICE_TS: &str = include_str!("../../../../web/src/data/service.ts");

fn page_methods() -> Vec<&'static str> {
    const NEEDLE: &str = "method: '";
    let mut methods = Vec::new();
    for (index, _) in SERVICE_TS.match_indices(NEEDLE) {
        let rest = &SERVICE_TS[index + NEEDLE.len()..];
        let Some(end) = rest.find('\'') else { continue };
        let method = &rest[..end];
        // `method: 'POST'` 是 HTTP 动词，不是协议方法。
        if method.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            methods.push(method);
        }
    }
    methods
}

#[test]
fn api_methods_are_call_subset() {
    let methods = page_methods();
    assert!(
        methods.len() >= 20,
        "service.ts 里解析出的方法太少，解析可能坏了：{methods:?}"
    );
    assert!(methods.contains(&"send"), "方法清单要含 send：{methods:?}");
    assert!(
        methods.contains(&"monitor-report"),
        "方法清单要含 monitor-report：{methods:?}"
    );
    for method in methods {
        if let Err(error) = serde_json::from_value::<Call>(serde_json::json!({"method": method})) {
            assert!(
                !error.to_string().contains("unknown variant"),
                "页面发的 /api 方法 {method} 不在 service::Call 里：{error}"
            );
        }
    }
}

#[test]
fn monitor_report_is_a_call() {
    let call: Call = serde_json::from_value(serde_json::json!({
        "method": "monitor-report",
        "kind": "page-error",
        "message": "boom",
    }))
    .expect("monitor-report 要能解码成 Call");
    let wire = serde_json::to_value(&call).expect("Call 序列化");
    assert_eq!(wire["method"], "monitor-report");
    assert_eq!(wire["kind"], "page-error");
    assert_eq!(wire["message"], "boom");
}
