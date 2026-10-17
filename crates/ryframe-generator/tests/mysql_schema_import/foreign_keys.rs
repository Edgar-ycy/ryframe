use ryframe_generator::import::{ForeignKeyAction, ForeignKeyMatch, inspect_existing_table};
use sea_orm::DatabaseConnection;

use super::mysql;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actions_and_composite_columns_preserve_canonical_schema_without_source_writes() {
    mysql::run_mysql_test("fk_actions", |database| async move {
        mysql::execute(
            &database,
            "CREATE TABLE biz_fk_parent (
                id BIGINT NOT NULL, tenant_key BIGINT NOT NULL,
                PRIMARY KEY(tenant_key, id)
            ) ENGINE=InnoDB",
        )
        .await?;
        mysql::execute(&database, "INSERT INTO biz_fk_parent VALUES (7, 10)").await?;
        for (name, update, delete, expected_update, expected_delete) in [
            (
                "cascade_restrict",
                "CASCADE",
                "RESTRICT",
                ForeignKeyAction::Cascade,
                ForeignKeyAction::Restrict,
            ),
            (
                "restrict_set_null",
                "RESTRICT",
                "SET NULL",
                ForeignKeyAction::Restrict,
                ForeignKeyAction::SetNull,
            ),
            (
                "set_null_cascade",
                "SET NULL",
                "CASCADE",
                ForeignKeyAction::SetNull,
                ForeignKeyAction::Cascade,
            ),
        ] {
            let table_name = format!("biz_fk_{name}");
            let constraint = format!("fk_{name}");
            mysql::execute(
                &database,
                &format!(
                    "CREATE TABLE {table_name} (
                        id BIGINT NOT NULL PRIMARY KEY,
                        parent_id BIGINT NULL, parent_tenant BIGINT NULL,
                        CONSTRAINT {constraint} FOREIGN KEY(parent_tenant, parent_id)
                        REFERENCES biz_fk_parent(tenant_key, id)
                        ON UPDATE {update} ON DELETE {delete}
                    ) ENGINE=InnoDB"
                ),
            )
            .await?;
            mysql::execute(
                &database,
                &format!("INSERT INTO {table_name} VALUES (1, 7, 10)"),
            )
            .await?;
            let before = ryframe_db::schema::canonical_table_schema(&database, &table_name)
                .await
                .map_err(mysql::db_error)?;
            let table = inspect_existing_table(&database, &table_name)
                .await
                .map_err(|error| error.to_string())?;
            assert_eq!(table.foreign_key_dependencies, ["biz_fk_parent"]);
            assert_eq!(table.foreign_keys.len(), 1);
            let key = &table.foreign_keys[0];
            assert_eq!(key.name, constraint);
            assert_eq!(key.columns, ["parent_tenant", "parent_id"]);
            assert_eq!(key.referenced_columns, ["tenant_key", "id"]);
            assert_eq!(key.referenced_table, "biz_fk_parent");
            assert_eq!(key.update_rule, expected_update);
            assert_eq!(key.delete_rule, expected_delete);
            assert_eq!(key.match_option, ForeignKeyMatch::None);
            for (position, column, referenced) in
                [(1, "parent_tenant", "tenant_key"), (2, "parent_id", "id")]
            {
                assert!(table.schema_canonical.contains(&format!(
                    "\"{constraint}\":\"{column}\":{position}:local:\"biz_fk_parent\":\
                     \"{referenced}\":\"{update}\":\"{delete}\";"
                )));
            }
            assert_eq!(table.schema_canonical, before);
            let metadata = serde_json::to_value(&table).map_err(|error| error.to_string())?;
            assert_eq!(metadata["foreign_keys"][0]["update_rule"], update);
            assert_eq!(metadata["foreign_keys"][0]["delete_rule"], delete);
            verify_source_rows(&database, &table_name).await?;
        }
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM biz_fk_parent WHERE id=7 AND tenant_key=10",
            1,
        )
        .await
    })
    .await;
}

async fn verify_source_rows(database: &DatabaseConnection, table_name: &str) -> Result<(), String> {
    mysql::require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM {table_name} \
             WHERE id=1 AND parent_id=7 AND parent_tenant=10"
        ),
        1,
    )
    .await
}
