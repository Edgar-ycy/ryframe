/// data_diff 模块测试
/// 从 crates/ryframe-common/src/utils/data_diff.rs 内联测试迁移
use ryframe_common::utils::data_diff::{DataDiff, DataDiffBuilder};
use serde_json::json;

#[test]
fn test_data_diff_json_no_changes() {
    let old = json!({"name": "张三", "age": 30});
    let new = json!({"name": "张三", "age": 30});
    let diff = DataDiff::from_json(&old, &new);
    assert!(!diff.has_changes);
    assert_eq!(diff.changed_count, 0);
}

#[test]
fn test_data_diff_json_with_changes() {
    let old = json!({"name": "张三", "age": 30, "status": "1"});
    let new = json!({"name": "李四", "age": 30, "status": "0"});
    let diff = DataDiff::from_json(&old, &new);
    assert!(diff.has_changes);
    assert_eq!(diff.changed_count, 2);

    assert_eq!(diff.format_text(), "[name] 张三 → 李四; [status] 1 → 0");
}

#[test]
fn test_data_diff_json_add_field() {
    let old = json!({"name": "张三"});
    let new = json!({"name": "张三", "email": "zhangsan@example.com"});
    let diff = DataDiff::from_json(&old, &new);
    assert!(diff.has_changes);
    assert_eq!(diff.changed_count, 1);
    assert!(diff.format_text().contains("(空) → zhangsan@example.com"));
}

#[test]
fn test_data_diff_json_remove_field() {
    let old = json!({"name": "张三", "email": "zhangsan@example.com"});
    let new = json!({"name": "张三"});
    let diff = DataDiff::from_json(&old, &new);
    assert!(diff.has_changes);
    assert_eq!(diff.changed_count, 1);
    assert!(diff.format_text().contains("zhangsan@example.com → (空)"));
}

#[test]
fn test_data_diff_builder() {
    let diff = DataDiffBuilder::new()
        .change("name", "张三", "李四")
        .change("status", "0", "1")
        .build();

    assert!(diff.has_changes);
    assert_eq!(diff.changed_count, 2);
    assert!(diff.format_text().contains("张三 → 李四"));
}

#[test]
fn test_data_diff_builder_no_changes() {
    let diff = DataDiffBuilder::new()
        .change("name", "张三", "张三")
        .build();

    assert!(!diff.has_changes);
    assert_eq!(diff.changed_count, 0);
}

#[test]
fn test_data_diff_serde_roundtrip() {
    let diff = DataDiffBuilder::new()
        .change("name", "old", "new")
        .change("status", "1", "0")
        .build();

    let json = diff.to_json_string();
    let restored = DataDiff::from_json_string(&json);

    assert_eq!(restored.changed_count, 2);
    assert!(restored.has_changes);
    assert_eq!(restored.changes.len(), 2);
}

#[test]
fn test_data_diff_empty() {
    let diff = DataDiff::new();
    assert!(!diff.has_changes);
    assert_eq!(diff.changed_count, 0);
    assert_eq!(diff.format_text(), "无变更");
}
