use std::{fs, path::Path};

use crate::import::{ForeignKeyAction, ForeignKeyInfo, ForeignKeyMatch, TableInfo};

#[test]
fn foreign_key_metadata_does_not_allow_installing_a_migration_without_the_constraint() {
    let cache =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/foreign-key-metadata");
    fs::create_dir_all(&cache).unwrap();
    let package = tempfile::tempdir_in(cache).unwrap();
    let history = package.path().join("src/generated/migrations/history.json");
    fs::create_dir_all(history.parent().unwrap()).unwrap();
    let generated = concat!(
        "v2|table=\"child\"|engine=\"innodb\"|charset=\"utf8mb4\"|collation=\"utf8mb4_general_ci\"",
        "|columns=[\"parent_id\":\"bigint\":\"YES\":None:None:\"\":None:\"\":\"\";]",
        "|indexes=[]|constraints=[]|checks=[]|foreign_keys=[]"
    );
    let original = serde_json::to_vec(&serde_json::json!({
        "resources": [{
            "descriptor": {"table": "child"},
            "versions": [{"canonical": generated}]
        }]
    }))
    .unwrap();
    fs::write(&history, &original).unwrap();
    let source = TableInfo {
        table_name: "child".into(),
        comment: None,
        collation: None,
        columns: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: vec![ForeignKeyInfo {
            name: "fk_child_parent".into(),
            columns: vec!["parent_id".into()],
            referenced_table: "parent".into(),
            referenced_columns: vec!["id".into()],
            update_rule: ForeignKeyAction::Cascade,
            delete_rule: ForeignKeyAction::SetNull,
            match_option: ForeignKeyMatch::None,
        }],
        foreign_key_dependencies: vec!["parent".into()],
        schema_canonical: generated.replace(
            "foreign_keys=[]",
            concat!(
                "foreign_keys=[\"fk_child_parent\":\"parent_id\":1:local:",
                "\"parent\":\"id\":\"CASCADE\":\"SET NULL\";]"
            ),
        ),
    };
    let error = super::verify_imported_schema(package.path(), &[source]).unwrap_err();
    assert!(
        error.contains("child") && error.contains("未表达的结构差异"),
        "{error}"
    );
    assert!(error.contains("本批未安装"), "{error}");
    #[cfg(feature = "schema-import")]
    assert!(error.contains("外键变化"), "{error}");
    assert_eq!(fs::read(history).unwrap(), original);
}
