use ryframe_utils::data_diff::{DataDiff, DataDiffBuilder};
use serde_json::json;

#[test]
fn identical_json_has_no_changes() {
    let old = json!({"name": "张三", "age": 30});
    let new = json!({"name": "张三", "age": 30});
    let diff = DataDiff::from_json(&old, &new);

    assert!(!diff.has_changes);
    assert_eq!(diff.changed_count, 0);
}

#[test]
fn changed_json_reports_each_changed_field() {
    let old = json!({"name": "张三", "age": 30, "status": "1"});
    let new = json!({"name": "李四", "age": 30, "status": "0"});
    let diff = DataDiff::from_json(&old, &new);

    assert!(diff.has_changes);
    assert_eq!(diff.changed_count, 2);
    assert_eq!(diff.format_text(), "[name] 张三 → 李四; [status] 1 → 0");
}

#[test]
fn added_and_removed_fields_are_visible() {
    let added = DataDiff::from_json(
        &json!({"name": "张三"}),
        &json!({"name": "张三", "email": "zhangsan@example.com"}),
    );
    assert_eq!(added.changed_count, 1);
    assert!(added.format_text().contains("(空) → zhangsan@example.com"));

    let removed = DataDiff::from_json(
        &json!({"name": "张三", "email": "zhangsan@example.com"}),
        &json!({"name": "张三"}),
    );
    assert_eq!(removed.changed_count, 1);
    assert!(
        removed
            .format_text()
            .contains("zhangsan@example.com → (空)")
    );
}

#[test]
fn builder_ignores_unchanged_values() {
    let diff = DataDiffBuilder::new()
        .change("name", "张三", "张三")
        .change("status", "0", "1")
        .build();

    assert!(diff.has_changes);
    assert_eq!(diff.changed_count, 1);
    assert_eq!(diff.format_text(), "[status] 0 → 1");
}

#[test]
fn serialized_diff_round_trips() {
    let diff = DataDiffBuilder::new()
        .change("name", "old", "new")
        .change("status", "1", "0")
        .build();
    let restored = DataDiff::from_json_string(&diff.to_json_string());

    assert_eq!(restored.changed_count, 2);
    assert!(restored.has_changes);
    assert_eq!(restored.changes.len(), 2);
}

#[test]
fn empty_diff_has_stable_text() {
    let diff = DataDiff::new();

    assert!(!diff.has_changes);
    assert_eq!(diff.changed_count, 0);
    assert_eq!(diff.format_text(), "无变更");
}
