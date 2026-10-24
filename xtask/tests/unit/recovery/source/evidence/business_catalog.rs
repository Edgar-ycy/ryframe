use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
    table: Value,
}

impl Fixture {
    fn new() -> Self {
        let parent = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let root = directory.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/order-business\"]\n",
        )
        .unwrap();
        let package = root.join("crates/order-business");
        fs::create_dir_all(package.join("src/generated")).unwrap();
        fs::create_dir(package.join(".ryframe")).unwrap();
        fs::write(package.join("Cargo.toml"), "[package]\nname = \"order-business\"\n[package.metadata.ryframe]\nkind = \"business\"\nmodule = \"order\"\n").unwrap();
        let fixture = Self {
            directory,
            table: json!({"table":"biz_order","copy_order":1,"tenant_column":"tenant_id",
                "primary_key_cursor_columns":["tenant_id","id"],
                "checksum_columns":["tenant_id","id","name"],
                "column_types":["varchar","bigint","varchar"],"has_generated_columns":false,
                "foreign_key_dependencies":[],"foreign_keys":[],"schema_canonical":"order-schema"}),
        };
        fixture.write_owned(json!({"format_version":1,"tables":[fixture.table]}));
        fixture
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn write_owned(&self, catalog: Value) {
        let package = self.root().join("crates/order-business");
        let resources = json!({"format_version":1,"module":"order",
            "resources":[{"table":"biz_order","database":"tenant"}]});
        let mut manifest = "format_version = 1\n".to_owned();
        for (relative, value) in [
            ("src/generated/tenant_catalog.json", catalog),
            ("src/generated/resources.json", resources),
        ] {
            let content = serde_json::to_vec(&value).unwrap();
            fs::write(package.join(relative), &content).unwrap();
            manifest.push_str(&format!(
                "[[entries]]\nresource = \"__catalog__\"\npath = \"{relative}\"\ncontent_hash = \"{}\"\n",
                hex::encode(Sha256::digest(&content))
            ));
        }
        fs::write(package.join(".ryframe/generated.toml"), manifest).unwrap();
    }
}

fn request(root: &Path, operation: &str) -> crate::Result<Value> {
    super::execute(&serde_json::to_vec(&json!({
        "operation":operation,"write":false,"arguments":{"backend":root}
    }))?)
}

#[test]
fn complete_owned_business_columns_are_included() {
    let fixture = Fixture::new();
    let result = request(fixture.root(), "business-catalog").unwrap();
    assert_eq!(result["tables"], json!([fixture.table]));
    assert_eq!(
        result["columns"],
        json!({"biz_order":[
            ["tenant_id","VARCHAR"],["id","BIGINT"],["name","VARCHAR"]
        ]})
    );
    let names = result["columns"]["biz_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| pair[0].clone())
        .collect::<Vec<_>>();
    assert_eq!(json!(names), fixture.table["checksum_columns"]);
}

#[test]
fn modified_generated_asset_is_rejected() {
    let fixture = Fixture::new();
    fs::write(
        fixture
            .root()
            .join("crates/order-business/src/generated/tenant_catalog.json"),
        "{}",
    )
    .unwrap();
    assert!(
        request(fixture.root(), "business-catalog")
            .unwrap_err()
            .to_string()
            .contains("受管资产")
    );
}

#[test]
fn resource_and_table_catalog_must_match() {
    let fixture = Fixture::new();
    fixture.write_owned(json!({"format_version":1,"tables":[]}));
    assert!(
        request(fixture.root(), "business-catalog")
            .unwrap_err()
            .to_string()
            .contains("资源与租户表")
    );
    fixture.write_owned(json!({"format_version":1,"tables":[fixture.table,fixture.table]}));
    assert!(
        request(fixture.root(), "business-catalog")
            .unwrap_err()
            .to_string()
            .contains("重复声明租户表")
    );
}

#[test]
fn incomplete_columns_cursor_and_schema_fail_closed() {
    let fixture = Fixture::new();
    for (field, value) in [
        ("checksum_columns", json!(["id"])),
        ("column_types", json!([])),
        ("primary_key_cursor_columns", json!(["unknown"])),
        ("schema_canonical", json!("")),
    ] {
        let mut table = fixture.table.clone();
        table[field] = value;
        fixture.write_owned(json!({"format_version":1,"tables":[table]}));
        assert!(
            request(fixture.root(), "business-catalog").is_err(),
            "{field}"
        );
    }
}

#[test]
fn unknown_workspace_path_is_rejected() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join("Cargo.toml"),
        "[workspace]\nmembers = [\"../unowned\"]\n",
    )
    .unwrap();
    assert!(
        request(fixture.root(), "business-catalog")
            .unwrap_err()
            .to_string()
            .contains("源码根目录")
    );
}

#[test]
fn business_ledger_columns_follow_formal_sdk_ddl() {
    let result = request(&crate::workspace::root_dir(), "business-ledger-columns").unwrap();
    assert_eq!(
        result,
        json!([
            ["scope", "VARCHAR"],
            ["module_name", "VARCHAR"],
            ["migration_id", "VARCHAR"],
            ["checksum", "CHAR"],
            ["before_fingerprint", "CHAR"],
            ["after_fingerprint", "CHAR"],
            ["origin", "VARCHAR"],
            ["tables_json", "JSON"],
            ["adoption_hash", "CHAR"],
            ["applied_at", "TIMESTAMP"]
        ])
    );
    let names = result
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| pair[0].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "scope",
            "module_name",
            "migration_id",
            "checksum",
            "before_fingerprint",
            "after_fingerprint",
            "origin",
            "tables_json",
            "adoption_hash",
            "applied_at"
        ]
    );
}

#[test]
fn business_ledger_rejects_ambiguous_or_incomplete_ddl() {
    let fixture = Fixture::new();
    let relative = "crates/ryframe-sdk/src/migration/ledger.rs";
    let source = fs::read_to_string(crate::workspace::root_dir().join(relative)).unwrap();
    let target = fixture.root().join(relative);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    for invalid in [
        format!("{source}{source}"),
        source.replace("tables_json JSON", "tables_json TEXT"),
        source.replace("const DDL:", "const UNKNOWN:"),
    ] {
        fs::write(&target, invalid).unwrap();
        assert!(request(fixture.root(), "business-ledger-columns").is_err());
    }
}
