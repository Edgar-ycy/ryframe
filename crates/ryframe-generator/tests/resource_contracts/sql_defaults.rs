use ryframe_generator::{ResourceSpec, normalize_resource, render_resources};

fn spec() -> ResourceSpec {
    ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap()
}

#[test]
fn database_clock_precision_ddl_and_canonical_metadata_stay_consistent() {
    for precision in [0, 3, 6] {
        let mut input = spec();
        let field = input
            .fields
            .iter_mut()
            .find(|field| field.name == "created_at")
            .unwrap();
        field.sql_type = Some(format!("datetime({precision})"));
        field.sql_default = Some(format!("current_timestamp({precision})"));
        field.sql_on_update = Some(format!("CURRENT_TIMESTAMP({precision})"));
        let resource = normalize_resource(input, "order.toml", "clock").unwrap();
        let expected = if precision == 0 {
            "CURRENT_TIMESTAMP".into()
        } else {
            format!("CURRENT_TIMESTAMP({precision})")
        };
        let generated = render_resources(&[resource]).unwrap();
        let migration = &generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with("order/migration.rs"))
            .unwrap()
            .content;
        assert!(migration.contains(&format!("DEFAULT {expected} ON UPDATE {expected}")));
        let catalog = &generated
            .assets
            .iter()
            .find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/catalog.rs")
            .unwrap()
            .content;
        assert!(catalog.contains(&format!("on update {}", expected.to_ascii_lowercase())));
    }
}

#[test]
fn unsupported_sql_functions_wrong_precision_and_ambiguous_defaults_fail_before_write() {
    for value in [
        "NOW()",
        "CURRENT_TIMESTAMP(7)",
        "CURRENT_TIMESTAMP(3)",
        "CURRENT_TIMESTAMP(); DROP TABLE table1",
        "(UUID())",
    ] {
        let mut input = spec();
        input
            .fields
            .iter_mut()
            .find(|field| field.name == "created_at")
            .unwrap()
            .sql_default = Some(value.into());
        let error = normalize_resource(input, "order.toml", "invalid-clock")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("created_at")
                && error.contains("本批未写入")
                && error.contains("修复建议")
        );
    }
    let mut input = spec();
    let field = input
        .fields
        .iter_mut()
        .find(|field| field.name == "created_at")
        .unwrap();
    field.default = Some(toml::Value::String("2026-10-15 00:00:00".into()));
    field.sql_default = Some("CURRENT_TIMESTAMP(6)".into());
    assert!(
        normalize_resource(input, "order.toml", "ambiguous")
            .unwrap_err()
            .to_string()
            .contains("不能同时声明")
    );
    let mut input = spec();
    input
        .fields
        .iter_mut()
        .find(|field| field.name == "name")
        .unwrap()
        .sql_default = Some("CURRENT_TIMESTAMP(6)".into());
    assert!(
        normalize_resource(input, "order.toml", "type")
            .unwrap_err()
            .to_string()
            .contains("仅支持 DATETIME")
    );
}
