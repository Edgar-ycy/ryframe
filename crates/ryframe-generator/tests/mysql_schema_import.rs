use ryframe_generator::import::inspect_existing_table;

#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;

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
