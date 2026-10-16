use sea_orm::{ConnectionTrait, DatabaseExecutor, DbBackend, Statement, TransactionTrait};

use super::mysql;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_and_exported_comments_preserve_text_in_both_sql_modes() {
    mysql::run_mysql_test("commentmodes", |database| async move {
        let comment = "中文 '注释'\\路径\n原样\"#; --";
        let quoted = format!("'{}'", comment.replace('\\', "\\\\").replace('\'', "''"));
        for (index, (no_backslash, export)) in [(false, false), (true, false), (false, true), (true, true)].into_iter().enumerate() {
            let pinned = database.begin().await.map_err(mysql::db_error)?;
            let original = pinned.query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT @@SESSION.sql_mode AS mode"))
                .await.map_err(mysql::db_error)?.ok_or("缺少 SQL 模式")?.try_get::<String>("", "mode").map_err(mysql::db_error)?;
            let mut modes = original.split(',').filter(|mode| *mode != "NO_BACKSLASH_ESCAPES").collect::<Vec<_>>();
            if no_backslash { modes.push("NO_BACKSLASH_ESCAPES"); }
            let configured = modes.join(",");
            pinned.execute_raw(Statement::from_sql_and_values(DbBackend::MySql, "SET SESSION sql_mode = ?", [configured.clone().into()])).await.map_err(mysql::db_error)?;
            let configured = pinned.query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT @@SESSION.sql_mode AS mode"))
                .await.map_err(mysql::db_error)?.ok_or("缺少已配置模式")?.try_get::<String>("", "mode").map_err(mysql::db_error)?;
            let table = format!("biz_comments_{index}");
            let sql = format!("CREATE TABLE `{table}` (id BIGINT PRIMARY KEY, value VARCHAR(32) COMMENT {quoted}) COMMENT={quoted}");
            if export {
                for statement in ryframe_db::connection::generated_mysql_ddl_statements(&sql) {
                    pinned.execute_unprepared(&statement).await.map_err(mysql::db_error)?;
                }
            } else {
                ryframe_db::connection::execute_generated_mysql_ddl(&DatabaseExecutor::from(&pinned), &sql).await.map_err(mysql::db_error)?;
            }
            let row = pinned.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
                "SELECT t.TABLE_COMMENT AS table_comment, c.COLUMN_COMMENT AS column_comment, @@SESSION.sql_mode AS mode FROM information_schema.tables t JOIN information_schema.columns c ON c.TABLE_SCHEMA=t.TABLE_SCHEMA AND c.TABLE_NAME=t.TABLE_NAME WHERE t.TABLE_SCHEMA=DATABASE() AND t.TABLE_NAME=? AND c.COLUMN_NAME='value'", [table.into()]))
                .await.map_err(mysql::db_error)?.ok_or("缺少注释表")?;
            assert_eq!(row.try_get::<String>("", "table_comment").map_err(mysql::db_error)?, comment);
            assert_eq!(row.try_get::<String>("", "column_comment").map_err(mysql::db_error)?, comment);
            assert_eq!(row.try_get::<String>("", "mode").map_err(mysql::db_error)?, configured, "执行器不得修改会话模式");
            pinned.execute_raw(Statement::from_sql_and_values(DbBackend::MySql, "SET SESSION sql_mode = ?", [original.into()])).await.map_err(mysql::db_error)?;
            pinned.commit().await.map_err(mysql::db_error)?;
        }
        Ok(())
    }).await;
}
