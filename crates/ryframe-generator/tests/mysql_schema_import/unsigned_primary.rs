use ryframe_generator::{
    IdGeneration, IndexSpec, ResourceSpec, ValueType, normalize_resource, render_resources,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};

use super::mysql;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_unsigned_tenant_identity_keeps_large_auto_ids_and_tenant_boundary() {
    mysql::run_mysql_test("unsigned_auto_id", |database| async move {
        let mut spec = ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
        spec.database.id_generation = IdGeneration::AutoIncrement;
        spec.database.indexes.push(IndexSpec { name: "idx_order_auto_id".into(), fields: vec!["id".into()], unique: false });
        spec.fields.iter_mut().find(|field| field.name == "id").unwrap().value_type = ValueType::U64;
        let resource = normalize_resource(spec, "order.toml", "unsigned-auto").unwrap();
        let generated = render_resources(&[resource]).unwrap();
        let migration = &generated.assets.iter().find(|asset| asset.path.ends_with("order/migration.rs")).unwrap().content;
        let ddl = migration.split_once("CREATE_TABLE_DDL: &str = r#\"").unwrap().1.split_once("\"#;").unwrap().0;
        mysql::execute(&database, ddl).await?;
        mysql::execute(&database, "ALTER TABLE biz_order AUTO_INCREMENT=9223372036854775808").await?;
        let transaction = database.begin().await.map_err(message)?;
        for (tenant, expected) in [("tenant-a", 9223372036854775808u64), ("tenant-b", 9223372036854775809u64)] {
            mysql::execute(&transaction, &format!("INSERT INTO biz_order(tenant_id,name,status,created_at,del_flag) VALUES ('{tenant}','first',1,NOW(6),0)")).await?;
            let row = transaction.query_one_raw(Statement::from_string(DbBackend::MySql, format!("SELECT id FROM biz_order WHERE tenant_id='{tenant}' AND id=LAST_INSERT_ID()"))).await.map_err(message)?.ok_or("自增记录不存在")?;
            assert_eq!(row.try_get::<u64>("", "id").map_err(message)?, expected);
        }
        mysql::require_count(&transaction, "SELECT COUNT(*) AS value FROM biz_order WHERE tenant_id='tenant-a' AND id=LAST_INSERT_ID()", 0).await?;
        mysql::execute(&transaction, "INSERT INTO biz_order(id,tenant_id,name,status,created_at,del_flag) VALUES (18446744073709551615,'tenant-a','maximum',1,NOW(6),0)").await?;
        let row = transaction.query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT id FROM biz_order WHERE tenant_id='tenant-a' AND name='maximum'")).await.map_err(message)?.ok_or("最大值记录不存在")?;
        assert_eq!(row.try_get::<u64>("", "id").map_err(message)?, u64::MAX);
        transaction.rollback().await.map_err(message)?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_order", 0).await?;
        let table = ryframe_generator::import::inspect_existing_table(&database, "biz_order").await.map_err(message)?;
        assert_eq!(table.columns.iter().find(|column| column.name == "id").unwrap().rust_type, "u64");
        let catalog = &generated.assets.iter().find(|asset| asset.path == "crates/ryframe-tenant-db/src/generated/catalog.rs").unwrap().content;
        assert!(catalog.contains(&format!("{:?}", table.schema_canonical)));
        Ok(())
    }).await;
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
