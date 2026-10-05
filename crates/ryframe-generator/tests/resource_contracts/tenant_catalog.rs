use std::path::PathBuf;

use ryframe_generator::{GeneratedCatalog, load_resource, render_resources};

fn asset(generated: &GeneratedCatalog, path: &str) -> String {
    generated
        .assets
        .iter()
        .find(|asset| asset.path == path)
        .expect("复制目录产物应存在")
        .content
        .clone()
}

const CATALOG: &str = "crates/ryframe-tenant-db/src/generated/catalog.rs";

fn fingerprint(catalog: &str) -> &str {
    catalog
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix('"')
                .and_then(|line| line.strip_suffix("\";"))
        })
        .expect("生成目录应包含独立 schema 指纹常量")
}

#[test]
fn tenant_catalog_keeps_empty_and_control_only_resources_valid() {
    let empty = render_resources(&[]).expect("空目录合法");
    assert!(asset(&empty, CATALOG).contains("= &[];"));
    let post = load_resource(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/resources/post.toml"),
    )
    .unwrap();
    let control = render_resources(&[post]).unwrap();
    assert_eq!(asset(&empty, CATALOG), asset(&control, CATALOG));
    assert!(asset(&empty, CATALOG).contains("GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT"));
    assert_eq!(
        fingerprint(&asset(&empty, CATALOG)),
        "a0a5cf5e7aae4ec0cdfe15765d450539dd32e7b7119846aac319ec98e122b04e"
    );
    assert!(
        asset(&control, "crates/ryframe-tenant-db/src/generated/mod.rs")
            .contains("pub mod catalog;")
    );
}

#[test]
fn order_catalog_includes_all_columns_and_tenant_cursor_without_control_tables() {
    let order =
        load_resource(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/order.toml"))
            .unwrap();
    let generated = render_resources(std::slice::from_ref(&order)).unwrap();
    let catalog = asset(&generated, CATALOG);
    for expected in [
        "table: \"biz_order\"",
        "primary_key_cursor_columns: &[\"tenant_id\", \"id\"]",
        "has_generated_columns: false",
        "foreign_key_dependencies: &[]",
        "foreign_keys: &[]",
    ] {
        assert!(catalog.contains(expected), "目录缺少 {expected}");
    }
    for field in &order.fields {
        assert!(catalog.contains(&format!("{:?}", field.column)));
    }
    assert!(!catalog.contains("sys_"));
    let mut changed = order;
    changed
        .fields
        .iter_mut()
        .find(|field| field.name == "status")
        .unwrap()
        .default = Some(toml::Value::Integer(0));
    let changed_output = render_resources(&[changed]).unwrap();
    assert_ne!(catalog, asset(&changed_output, CATALOG));
    assert_ne!(
        fingerprint(&catalog),
        fingerprint(&asset(&changed_output, CATALOG))
    );
    assert_ne!(
        asset(
            &generated,
            "crates/ryframe-tenant-db/src/generated/order/migration.rs"
        ),
        asset(
            &changed_output,
            "crates/ryframe-tenant-db/src/generated/order/migration.rs"
        )
    );
}

#[test]
fn catalog_copy_order_is_deterministic_for_multiple_resources() {
    let order =
        load_resource(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/order.toml"))
            .unwrap();
    let mut meter = order.clone();
    meter.name = "meter".into();
    meter.pascal_name = "Meter".into();
    meter.table = "biz_meter".into();
    let before = render_resources(&[meter.clone(), order.clone()]).unwrap();
    let after = render_resources(&[order, meter]).unwrap();
    let catalog = asset(&before, CATALOG);
    assert_eq!(catalog, asset(&after, CATALOG));
    assert!(catalog.find("biz_meter").unwrap() < catalog.find("biz_order").unwrap());
    assert!(catalog.contains("copy_order: 1"));
    assert!(catalog.contains("copy_order: 2"));
    assert!(!catalog.contains("copy_order: 0"));
}
