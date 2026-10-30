use super::*;
use native::{business, catalog, export::database};

const BUSINESS_LEDGER: &str = "ryframe_business_migration";

fn catalog_root(fixture: &Fixture, with_business: bool) -> PathBuf {
    let root = fixture.local.join("catalog-input");
    for relative in [
        "sql/ryframe_config.sql",
        "crates/ryframe-tenant-db/src/generated/catalog.rs",
        "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs",
        "crates/ryframe-sdk/src/migration/ledger.rs",
    ] {
        let target = root.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(fixture.backend.join(relative), target).unwrap();
    }
    let members = if with_business {
        "[\"crates/order-business\"]"
    } else {
        "[]"
    };
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = {members}\n"),
    )
    .unwrap();
    if with_business {
        write_business_catalog(&root);
    }
    root
}

fn write_business_catalog(root: &Path) {
    let package = root.join("crates/order-business");
    fs::create_dir_all(package.join("src/generated")).unwrap();
    fs::create_dir(package.join(".ryframe")).unwrap();
    fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"order-business\"\nversion = \"0.1.0\"\n\
         [package.metadata.ryframe]\nkind = \"business\"\nmodule = \"order\"\n",
    )
    .unwrap();
    let catalog = json!({"format_version":1,"tables":[{
        "table":"biz_order","copy_order":1,"tenant_column":"tenant_id",
        "primary_key_cursor_columns":["tenant_id","id"],
        "checksum_columns":["tenant_id","id","name"],
        "column_types":["varchar","bigint","varchar"],"has_generated_columns":false,
        "foreign_key_dependencies":[],"foreign_keys":[],"schema_canonical":"order-schema"
    }]});
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
            sha(&content)
        ));
    }
    fs::write(package.join(".ryframe/generated.toml"), manifest).unwrap();
}

#[test]
fn export_models_include_owned_business_columns_and_complete_sdk_ledger_only_when_needed() {
    for with_business in [false, true] {
        let fixture = Fixture::new();
        let mut context = object_capture_context(&fixture);
        context.execution_backend = catalog_root(&fixture, with_business);
        let models = database::models(&context).unwrap();
        if with_business {
            let order = serde_json::from_value::<catalog::Columns>(json!({
                "tenant_id":"VARCHAR","id":"BIGINT","name":"VARCHAR"
            }))
            .unwrap();
            assert_eq!(models.tenant.get("biz_order"), Some(&order));
            assert_eq!(models.full_tenant.get("biz_order"), Some(&order));
            // 以正式 SDK DDL 为准，避免测试复制一份可能过时的账本列清单。
            let ledger = business::ledger_columns(&context.execution_backend).unwrap();
            assert!(!ledger.is_empty());
            assert_eq!(models.full_tenant.get(BUSINESS_LEDGER), Some(&ledger));
        } else {
            assert!(!models.tenant.contains_key("biz_order"));
            assert!(!models.full_tenant.contains_key("biz_order"));
            assert!(!models.full_tenant.contains_key(BUSINESS_LEDGER));
        }
    }
}

// 模拟 information_schema 的实际返回，独立于待测 models 生成，防止遗漏自洽。
fn actual_columns(root: &Path, combined: bool) -> catalog::Catalog {
    let loaded = catalog::load_ordered(root, true).unwrap();
    let (control, tenant) = loaded.full.unwrap();
    let mut ordered = if combined {
        control
    } else {
        Default::default()
    };
    ordered.extend(tenant);
    for ledger in if combined {
        vec!["seaql_migrations", "seaql_tenant_data_migrations"]
    } else {
        vec!["seaql_tenant_data_migrations"]
    } {
        ordered.insert(
            ledger.into(),
            vec![
                ("version".into(), "VARCHAR".into()),
                ("applied_at".into(), "BIGINT".into()),
            ],
        );
    }
    ordered
        .into_iter()
        .map(|(table, columns)| (table, columns.into_iter().collect()))
        .collect()
}

struct SchemaTransport {
    columns: catalog::Catalog,
}

impl Transport for SchemaTransport {
    fn run(&self, spec: CommandSpec) -> crate::Result<CommandOutput> {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;

        assert!(
            spec.args
                .last()
                .unwrap()
                .contains("FROM information_schema.COLUMNS")
        );
        let mut stdout = String::new();
        for (table, columns) in &self.columns {
            for (column, kind) in columns {
                stdout.push_str(&format!("{table}\t{column}\t{kind}\n"));
            }
        }
        Ok(CommandOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.into_bytes(),
            stderr: Vec::new(),
        })
    }

    fn s3(&self, _: native::s3::Request) -> crate::Result<Value> {
        panic!("schema 快照不得访问对象服务")
    }
}

#[test]
fn schema_snapshot_accepts_full_catalog_and_rejects_unknown_tables_missing_columns_and_type_drift()
{
    for with_business in [false, true] {
        let fixture = Fixture::new();
        let mut context = object_capture_context(&fixture);
        context.execution_backend = catalog_root(&fixture, with_business);
        let models = database::models(&context).unwrap();
        for combined in [false, true] {
            let kind = if combined { "combined" } else { "dedicated" };
            context.request["source"]["databases"][0]["kind"] = json!(kind);
            context.request["source"]["databases"][0]["key"] = json!(kind);
            let actual = actual_columns(&context.execution_backend, combined);
            for drift in ["none", "unknown-table", "missing-column", "column-type"] {
                context.output_dir = fixture.run.join("e0001").join(format!("{kind}-{drift}"));
                fs::create_dir(&context.output_dir).unwrap();
                let mut columns = actual.clone();
                let table = if with_business {
                    BUSINESS_LEDGER
                } else {
                    "biz_tenant_fence"
                };
                match drift {
                    "unknown-table" => {
                        columns.insert(
                            "unregistered_table".into(),
                            catalog::Columns::from([("id".into(), "BIGINT".into())]),
                        );
                    }
                    "missing-column" | "column-type" => {
                        let table = columns.get_mut(table).unwrap();
                        let column = table.keys().next().unwrap().clone();
                        if drift == "missing-column" {
                            table.remove(&column);
                        } else {
                            table.insert(column, "BLOB".into());
                        }
                    }
                    _ => {}
                }
                let before = snapshot(&context.output_dir);
                let result = database::schema_snapshot(
                    &context,
                    &models,
                    &SchemaTransport { columns },
                    "before",
                );
                let label = format!("business={with_business}, {kind}, {drift}");
                if drift == "none" {
                    let rows = result.unwrap_or_else(|error| panic!("{label}: {error}"));
                    assert_eq!(rows.len(), 1, "{label}");
                    assert_eq!(rows[0]["key"], kind, "{label}");
                    assert_eq!(rows[0]["columns"], json!(actual), "{label}");
                    let evidence: Value = serde_json::from_slice(
                        &fs::read(context.output().join(format!("schema-before-{kind}.json")))
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(evidence["columns"], json!(actual), "{label}");
                } else {
                    let error = result.unwrap_err().to_string();
                    assert_eq!(
                        error, "源完整表或列与当前 schema catalog 不符，不能过滤未知表",
                        "{label}"
                    );
                    assert_eq!(snapshot(&context.output_dir), before, "{label}");
                }
            }
        }
    }
}
