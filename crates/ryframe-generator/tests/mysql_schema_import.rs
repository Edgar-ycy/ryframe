use ryframe_generator::import::{inspect_existing_table, plan_import_models};
use sea_orm::TransactionTrait;

#[path = "mysql_schema_import/business_evolution.rs"]
mod business_evolution;
#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;
#[path = "mysql_schema_import/unsigned_primary.rs"]
mod unsigned_primary;
#[path = "mysql_schema_import/unsupported_indexes.rs"]
mod unsupported_indexes;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_tables_without_tenant_prefix_share_introspection_but_keep_tenant_boundary() {
    mysql::run_mysql_test("global_schema", |database| async move {
        mysql::execute(&database, "CREATE TABLE table1 (id BIGINT NOT NULL PRIMARY KEY, name VARCHAR(32) NOT NULL)").await?;
        mysql::execute(&database, "CREATE TABLE table2 (id BIGINT NOT NULL PRIMARY KEY, parent_id BIGINT NOT NULL, CONSTRAINT fk_table2_parent FOREIGN KEY(parent_id) REFERENCES table1(id))").await?;
        for name in ["table1", "table2"] {
            let table = inspect_existing_table(&database, name).await.map_err(|error| error.to_string())?;
            assert_eq!(table.table_name, name);
            assert!(table.schema_canonical.contains(&format!("table={name:?}")));
            assert!(ryframe_tenant_db::migration::canonical_table_schema(&database, name).await.is_err());
        }
        let second = inspect_existing_table(&database, "table2").await.map_err(|error| error.to_string())?;
        assert_eq!(second.foreign_keys[0].referenced_table, "table1");
        assert!(second.schema_canonical.contains("fk_table2_parent"));
        for invalid in ["", "table1;DROP", "../table1"] {
            assert!(ryframe_db::schema::canonical_table_schema(&database, invalid).await.is_err());
        }
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name IN ('table1','table2')", 2).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_tenant_primary_index_order_is_independent_of_physical_column_order() {
    mysql::run_mysql_test("schema_pk_order", |database| async move {
        mysql::execute(&database, "CREATE TABLE biz_ordered (order_id BIGINT NOT NULL AUTO_INCREMENT, name VARCHAR(32) NOT NULL, tenant_id VARCHAR(36) NOT NULL, PRIMARY KEY(tenant_id,order_id), KEY idx_ordered_id(order_id))").await?;
        let table = inspect_existing_table(&database, "biz_ordered").await.map_err(|error| error.to_string())?;
        assert_eq!(table.columns[0].name, "order_id");
        let primary = table.indexes.iter().find(|index| index.name == "PRIMARY").unwrap();
        assert_eq!(primary.columns, ["tenant_id", "order_id"]);
        let model = ryframe_generator::import::rust_model_source(&table, "tenant")?;
        assert!(model.contains("primary_key = [\"tenant_id\", \"id\"]"));
        assert!(model.contains("column = \"order_id\", primary_key, auto_increment"));
        assert!(model.find("pub id:").unwrap() < model.find("pub tenant_id:").unwrap());
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_ordered", 0).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_tenant_auto_increment_ddl_and_transaction_readback_match_mysql() {
    use ryframe_generator::{
        IdGeneration, IndexSpec, ResourceSpec, normalize_resource, render_resources,
    };
    mysql::run_mysql_test("schema_auto_id", |database| async move {
        let mut spec = ResourceSpec::parse(include_str!("fixtures/order.toml"), "order.toml").unwrap();
        spec.database.id_generation = IdGeneration::AutoIncrement;
        spec.database.indexes.push(IndexSpec {
            name: "idx_order_auto_id".into(), fields: vec!["id".into()], unique: false,
        });
        let resource = normalize_resource(spec, "order.toml", "auto").unwrap();
        let generated = render_resources(&[resource]).unwrap();
        let migration = &generated.assets.iter().find(|asset| asset.path.ends_with("order/migration.rs")).unwrap().content;
        let ddl = migration.split_once("CREATE_TABLE_DDL: &str = r#\"").unwrap().1.split_once("\"#;").unwrap().0;
        mysql::execute(&database, ddl).await?;
        let transaction = database.begin().await.map_err(|error| error.to_string())?;
        mysql::execute(&transaction, "SET SESSION sql_mode = CONCAT(@@sql_mode, ',NO_AUTO_VALUE_ON_ZERO')").await?;
        mysql::execute(&transaction, "INSERT INTO biz_order(tenant_id, name, status, created_at, del_flag) VALUES ('tenant-a','first',1,NOW(6),0)").await?;
        mysql::require_count(&transaction, "SELECT COUNT(*) AS value FROM biz_order WHERE tenant_id='tenant-a' AND id=LAST_INSERT_ID() AND id=1", 1).await?;
        mysql::execute(&transaction, "INSERT INTO biz_order(tenant_id, name, status, created_at, del_flag) VALUES ('tenant-b','first',1,NOW(6),0)").await?;
        mysql::require_count(&transaction, "SELECT COUNT(*) AS value FROM biz_order WHERE tenant_id='tenant-b' AND id=LAST_INSERT_ID() AND id=2", 1).await?;
        mysql::require_count(&transaction, "SELECT COUNT(*) AS value FROM biz_order WHERE tenant_id='tenant-a' AND id=LAST_INSERT_ID()", 0).await?;
        transaction.rollback().await.map_err(|error| error.to_string())?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_order", 0).await?;
        let table = inspect_existing_table(&database, "biz_order").await.map_err(|error| error.to_string())?;
        let catalog = &generated.assets.iter().find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/catalog.rs").unwrap().content;
        assert!(catalog.contains(&format!("{:?}", table.schema_canonical)), "生成器物理结构描述必须与实际 MySQL DDL 完全一致");
        assert!(table.columns.iter().find(|column| column.name == "id").unwrap().is_auto_increment);
        assert!(table.indexes.iter().any(|index| index.name == "idx_order_auto_id" && !index.unique && index.columns == ["id"]));
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_tables_preserve_types_defaults_indexes_and_foreign_keys() {
    mysql::run_mysql_test("schema_import", |database| async move {
        mysql::execute(
            &database,
            "CREATE TABLE biz_order (
            id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
            code VARCHAR(64) NOT NULL UNIQUE,
            amount DECIMAL(20,6) NOT NULL DEFAULT 0.125000,
            payload JSON NULL,
            created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
            INDEX idx_code_amount(code, amount)
        )",
        )
        .await?;
        mysql::execute(
            &database,
            "CREATE TABLE biz_order_item (
            id BIGINT NOT NULL PRIMARY KEY, order_id BIGINT UNSIGNED NOT NULL,
            CONSTRAINT fk_order_item FOREIGN KEY (order_id) REFERENCES biz_order(id)
        )",
        )
        .await?;
        mysql::execute(
            &database,
            "INSERT INTO biz_order(code) VALUES ('keep-source')",
        )
        .await?;
        let order = inspect_existing_table(&database, "biz_order")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(order.columns[0].rust_type, "u64");
        assert!(order.columns[0].is_auto_increment);
        assert_eq!(order.columns[2].column_type, "decimal(20,6)");
        assert_eq!(order.columns[2].default.as_deref(), Some("0.125000"));
        assert!(
            order.columns[4]
                .extra
                .to_ascii_lowercase()
                .contains("default_generated")
        );
        assert!(
            order
                .indexes
                .iter()
                .any(|index| index.columns == ["code", "amount"])
        );
        let item = inspect_existing_table(&database, "biz_order_item")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(item.foreign_keys[0].referenced_table, "biz_order");
        assert_eq!(item.foreign_keys[0].columns, ["order_id"]);
        let workspace = tempfile::Builder::new()
            .prefix("mysql-import-batch-")
            .tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(workspace.path().join("src/resources"))
            .map_err(|error| error.to_string())?;
        std::fs::write(
            workspace.path().join("src/resources/mod.rs"),
            "pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> { vec![] }\n",
        )
        .map_err(|error| error.to_string())?;
        let batch = plan_import_models(
            workspace.path(),
            workspace.path(),
            &[order, item],
            "control",
            Some("biz_"),
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(batch.created.len(), 4);
        batch.write().map_err(|error| error.to_string())?;
        let model = std::fs::read_to_string(workspace.path().join("src/resources/order.rs"))
            .map_err(|error| error.to_string())?;
        assert!(
            model.contains("ryframe_sdk::rust_decimal::Decimal") && model.contains("pub id: u64")
        );
        let registration = std::fs::read_to_string(workspace.path().join("src/resources/mod.rs"))
            .map_err(|error| error.to_string())?;
        assert!(
            registration.contains("order::Order") && registration.contains("order_item::OrderItem")
        );
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM biz_order WHERE code='keep-source'",
            1,
        )
        .await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_missing_tables_and_unknown_fields_are_actionable_errors() {
    mysql::run_mysql_test("schema_errors", |database| async move {
        mysql::execute(
            &database,
            "CREATE TABLE biz_geometry (id BIGINT PRIMARY KEY, shape GEOMETRY NOT NULL)",
        )
        .await?;
        mysql::execute(
            &database,
            "CREATE VIEW biz_view AS SELECT id FROM biz_geometry",
        )
        .await?;
        let error = inspect_existing_table(&database, "biz_geometry")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("biz_geometry") && error.contains("shape") && error.contains("geometry")
        );
        for name in ["biz_view", "missing_table"] {
            assert!(
                inspect_existing_table(&database, name)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("普通表")
            );
        }
        Ok(())
    })
    .await;
}
