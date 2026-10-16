use sea_orm::{
    ConnectionTrait, DatabaseExecutor, DatabaseTransaction, DbBackend, DbErr, Statement,
    TransactionTrait,
};
use std::fmt::Write;

/// 执行生成器按常规反斜杠模式转义的 DDL；固定连接读取模式，不修改会话设置。
/// MySQL DDL 会隐式提交，此函数不承诺结构写入可以回滚，也不会自动重试。
pub async fn execute_generated_mysql_ddl(
    connection: &DatabaseExecutor<'_>,
    sql: &str,
) -> Result<(), DbErr> {
    if connection.get_database_backend() != DbBackend::MySql {
        return Err(DbErr::Custom("生成 DDL 只支持 MySQL".into()));
    }
    match connection {
        DatabaseExecutor::Connection(database) => {
            let pinned = database.begin().await?;
            execute_pinned(&pinned, sql).await?;
            pinned.commit().await.map_err(|error| DbErr::Custom(format!("DDL 已提交至服务器，结束连接上下文失败：{error}；先核对实际结构与账本，不自动重放")))
        }
        DatabaseExecutor::Transaction(transaction) => execute_pinned(transaction, sql).await,
        DatabaseExecutor::OwnedTransaction(transaction) => execute_pinned(transaction, sql).await,
    }
}

async fn execute_pinned(connection: &DatabaseTransaction, sql: &str) -> Result<(), DbErr> {
    let mode = connection
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@SESSION.sql_mode AS sql_mode",
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("无法读取当前连接 SQL 模式；未执行 DDL".into()))?
        .try_get::<String>("", "sql_mode")?;
    let sql = if mode.split(',').any(|mode| mode == "NO_BACKSLASH_ESCAPES") {
        sql.replace("\\\\", "\\")
    } else {
        sql.to_owned()
    };
    connection.execute_unprepared(&sql).await.map(|_| ())
}

/// SQL 文件在客户端的同一会话中选择转义形式；十六进制仅用于传递语句文本。
/// 输入限于生成器 DDL，反斜杠只出现在已经成对转义的注释字面量中。
pub fn generated_mysql_ddl_statements(sql: &str) -> Vec<String> {
    if !sql.contains("\\\\") {
        return vec![sql.into()];
    }
    let mut text = String::from("CONVERT(X'");
    for byte in sql.bytes() {
        write!(text, "{byte:02x}").expect("写入 String 不会失败");
    }
    text.push_str("' USING utf8mb4)");
    vec![
        format!(
            "SET @ryframe_initialization_ddl = IF(FIND_IN_SET('NO_BACKSLASH_ESCAPES', @@SESSION.sql_mode), REPLACE({text}, CHAR(92,92), CHAR(92)), {text})"
        ),
        "PREPARE ryframe_initialization_statement FROM @ryframe_initialization_ddl".into(),
        "EXECUTE ryframe_initialization_statement".into(),
        "DEALLOCATE PREPARE ryframe_initialization_statement".into(),
        "SET @ryframe_initialization_ddl = NULL".into(),
    ]
}
