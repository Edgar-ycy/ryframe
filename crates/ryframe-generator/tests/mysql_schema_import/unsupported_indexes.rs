use super::mysql;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_index_semantics_fail_before_importing_a_lossy_model() {
    mysql::run_mysql_test("index_semantics", |database| async move {
        for (table, index) in [
            ("biz_prefix", "KEY idx_prefix(code(8))"),
            ("biz_desc", "KEY idx_desc(code DESC)"),
            ("biz_hidden", "KEY idx_hidden(code) INVISIBLE"),
            ("biz_function", "KEY idx_function((LOWER(code)))"),
            ("biz_fulltext", "FULLTEXT KEY idx_fulltext(code)"),
        ] {
            mysql::execute(&database, &format!("CREATE TABLE {table} (id BIGINT PRIMARY KEY, code VARCHAR(32) NOT NULL, {index})")).await?;
            let error = ryframe_generator::import::inspect_existing_table(&database, table).await.unwrap_err().to_string();
            assert!(error.contains(table) && error.contains("索引") && error.contains("本批未写入"), "{error}");
            mysql::require_count(&database, &format!("SELECT COUNT(*) AS value FROM {table}"), 0).await?;
        }
        Ok(())
    }).await;
}
