use ryframe_generator::import::{ForeignKeyAction, ForeignKeyInfo, ForeignKeyMatch};
use serde_json::json;

#[test]
fn foreign_key_metadata_preserves_sql_actions_and_composite_column_order() {
    let foreign_key = ForeignKeyInfo {
        name: "fk_item_order".into(),
        columns: vec!["tenant_id".into(), "order_id".into()],
        referenced_table: "biz_order".into(),
        referenced_columns: vec!["tenant_id".into(), "id".into()],
        update_rule: ForeignKeyAction::Cascade,
        delete_rule: ForeignKeyAction::SetNull,
        match_option: ForeignKeyMatch::None,
    };
    assert_eq!(
        serde_json::to_value(foreign_key).unwrap(),
        json!({
            "name": "fk_item_order",
            "columns": ["tenant_id", "order_id"],
            "referenced_table": "biz_order",
            "referenced_columns": ["tenant_id", "id"],
            "update_rule": "CASCADE",
            "delete_rule": "SET NULL",
            "match_option": "NONE"
        })
    );
}

#[test]
fn known_actions_round_trip_without_collapsing_no_action_into_restrict() {
    for (sql, action) in [
        ("CASCADE", ForeignKeyAction::Cascade),
        ("RESTRICT", ForeignKeyAction::Restrict),
        ("SET NULL", ForeignKeyAction::SetNull),
        ("NO ACTION", ForeignKeyAction::NoAction),
        ("SET DEFAULT", ForeignKeyAction::SetDefault),
    ] {
        assert_eq!(sql.parse::<ForeignKeyAction>().unwrap(), action);
        assert_eq!(
            format!(" {} ", sql.to_ascii_lowercase())
                .parse::<ForeignKeyAction>()
                .unwrap(),
            action
        );
        assert_eq!(serde_json::to_value(action).unwrap(), json!(sql));
        assert_eq!(
            serde_json::from_value::<ForeignKeyAction>(json!(sql)).unwrap(),
            action
        );
    }
}

#[test]
fn known_match_options_round_trip_as_reported_metadata() {
    for (sql, rule) in [
        ("NONE", ForeignKeyMatch::None),
        ("SIMPLE", ForeignKeyMatch::Simple),
        ("FULL", ForeignKeyMatch::Full),
        ("PARTIAL", ForeignKeyMatch::Partial),
    ] {
        assert_eq!(sql.parse::<ForeignKeyMatch>().unwrap(), rule);
        assert_eq!(serde_json::to_value(rule).unwrap(), json!(sql));
        assert_eq!(
            serde_json::from_value::<ForeignKeyMatch>(json!(sql)).unwrap(),
            rule
        );
    }
}

#[test]
fn unknown_or_malformed_rules_are_rejected_without_a_string_fallback() {
    for value in [
        "",
        "FUTURE ACTION",
        "SET_NULL",
        "CASCADE; DROP TABLE parent",
    ] {
        assert!(
            value
                .parse::<ForeignKeyAction>()
                .unwrap_err()
                .contains("未知外键引用动作")
        );
        assert!(serde_json::from_value::<ForeignKeyAction>(json!(value)).is_err());
    }
    for value in ["", "FUTURE MATCH", "NONE; DROP TABLE parent"] {
        assert!(
            value
                .parse::<ForeignKeyMatch>()
                .unwrap_err()
                .contains("未知外键匹配规则")
        );
        assert!(serde_json::from_value::<ForeignKeyMatch>(json!(value)).is_err());
    }
    for value in [json!(null), json!(1), json!({"action": "CASCADE"})] {
        assert!(serde_json::from_value::<ForeignKeyAction>(value.clone()).is_err());
        assert!(serde_json::from_value::<ForeignKeyMatch>(value).is_err());
    }
}
