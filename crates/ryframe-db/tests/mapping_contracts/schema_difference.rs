use ryframe_db::schema::{describe_schema_difference, validate_canonical_schema};

const SCHEMA: &str = r#"v2|table="table1"|engine="innodb"|charset="utf8mb4"|collation="utf8mb4_general_ci"|columns=["id":"bigint":"NO":None:None:"PRI":None:"":"";"code":"varchar(32)":"NO":Some("utf8mb4"):Some("utf8mb4_general_ci"):"":Some("sensitive|value;with:separators\"quoted"):"":"";]|indexes=["PRIMARY":"id":1:0:"btree":None:"YES";]|constraints=["PRIMARY":"PRIMARY KEY":"YES";]|checks=[]|foreign_keys=[]"#;

#[test]
fn malformed_or_wrong_target_schema_is_rejected_before_ddl() {
    assert!(validate_canonical_schema("table1", SCHEMA));
    assert!(!validate_canonical_schema("table2", SCHEMA));
    assert!(!validate_canonical_schema(
        "table1",
        "v2|table=\"table1\"|bad"
    ));
    assert!(!validate_canonical_schema(
        "table1",
        &SCHEMA.replace("v2|", "v3|")
    ));
    assert!(!validate_canonical_schema(
        "table1",
        &SCHEMA.replace("|foreign_keys=[]", "")
    ));
}

#[test]
fn schema_diagnostics_name_changed_attributes_without_echoing_values() {
    assert_eq!(describe_schema_difference(SCHEMA, SCHEMA), "结构一致");
    let changed = SCHEMA
        .replace("varchar(32)", "varchar(64)")
        .replace("sensitive|value", "another-secret|value");
    let diagnostic = describe_schema_difference(SCHEMA, &changed);
    assert!(
        diagnostic.contains("列 code 的类型、默认值不同"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("sensitive") && !diagnostic.contains("another-secret"));
    assert!(!diagnostic.contains("列顺序"));
}

#[test]
fn schema_diagnostics_distinguish_added_removed_columns_and_index_changes() {
    let added = SCHEMA.replace(
        "]|indexes=",
        "\"extra\":\"int\":\"YES\":None:None:\"\":None:\"\":\"\";]|indexes=",
    );
    assert!(describe_schema_difference(SCHEMA, &added).contains("新增列 extra"));
    assert!(describe_schema_difference(&added, SCHEMA).contains("缺少列 extra"));
    let index = SCHEMA.replace("PRIMARY\":\"id\":1:0", "PRIMARY\":\"code\":1:0");
    assert!(describe_schema_difference(SCHEMA, &index).contains("索引变化：PRIMARY"));
    let engine = SCHEMA.replace("innodb", "myisam");
    assert!(describe_schema_difference(SCHEMA, &engine).contains("存储引擎不同"));
}
