//! 用量显示：厂商没给的项是「未知」，不是 0（测试清单第 10 条）。

use model::Usage;

#[test]
fn missing_usage_fields_are_unknown_not_zero() {
    let partial = Usage {
        input_tokens: Some(10),
        output_tokens: None,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: Some(0),
    };
    assert_eq!(
        partial.summary(),
        "输入 10 · 输出 未知 · 缓存读 未知 · 缓存写 0"
    );
    assert!(!partial.summary().contains("输出 0"));

    let empty = Usage::default();
    assert_eq!(
        empty.summary(),
        "输入 未知 · 输出 未知 · 缓存读 未知 · 缓存写 未知"
    );
    assert_eq!(empty.summary().matches("未知").count(), 4);
}
