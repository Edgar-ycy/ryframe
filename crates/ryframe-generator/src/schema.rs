use std::collections::HashSet;

use ryframe_common::{AppError, AppResult};
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub rust_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_auto_increment: bool,
    pub comment: Option<String>,
}

/// 读取单张表的结构信息
pub async fn fetch_table(db: &DatabaseConnection, table_name: &str) -> AppResult<TableInfo> {
    // 验证表名只包含字母、数字和下划线，防止注入
    if !table_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ryframe_common::AppError::Validation(
            "表名只能包含字母、数字和下划线".into(),
        ));
    }

    let columns = query_columns(db, table_name).await?;

    let mut col_infos: Vec<ColumnInfo> = Vec::new();
    for col in columns {
        let rust_type = crate::type_mapping::db_to_rust(&col.data_type, col.is_nullable == "YES");
        let col_info = ColumnInfo {
            name: col.column_name.clone(),
            data_type: col.data_type.clone(),
            rust_type: rust_type.to_string(),
            is_nullable: col.is_nullable == "YES",
            is_primary_key: col.column_key == "PRI",
            is_unique: col.column_key == "UNI",
            is_auto_increment: col.extra.contains("auto_increment"),
            comment: col.column_comment,
        };
        col_infos.push(col_info);
    }

    let table_comment = query_table_comment(db, table_name).await?;

    Ok(TableInfo {
        table_name: table_name.to_string(),
        comment: table_comment,
        columns: col_infos,
    })
}

/// 列出数据库中所有表
pub async fn list_tables(db: &DatabaseConnection) -> AppResult<Vec<String>> {
    let tables = query_tables(db).await?;
    Ok(tables.into_iter().map(|t| t.table_name).collect())
}

/// 获取主键的 Rust 类型（通用工具函数）
pub fn get_pk_type(table: &TableInfo) -> &str {
    table
        .columns
        .iter()
        .find(|column| column.is_primary_key)
        .map(|column| column.rust_type.as_str())
        .unwrap_or("i64")
}

#[derive(Debug, FromQueryResult)]
struct ColumnRow {
    column_name: String,
    data_type: String,
    is_nullable: String,
    column_key: String,
    extra: String,
    column_comment: Option<String>,
}

async fn query_columns(db: &DatabaseConnection, table_name: &str) -> AppResult<Vec<ColumnRow>> {
    let backend = db.get_database_backend();
    match backend {
        DatabaseBackend::MySql => {
            query_columns_with_sql(
                db,
                table_name,
                r#"SELECT COLUMN_NAME AS column_name,
                          DATA_TYPE AS data_type,
                          IS_NULLABLE AS is_nullable,
                          COLUMN_KEY AS column_key,
                          EXTRA AS extra,
                          NULLIF(COLUMN_COMMENT, '') AS column_comment
                   FROM information_schema.columns
                   WHERE table_schema = DATABASE() AND table_name = ?
                   ORDER BY ORDINAL_POSITION"#,
            )
            .await
        }
        DatabaseBackend::Postgres => {
            query_columns_with_sql(
                db,
                table_name,
                r#"SELECT c.column_name,
                          c.udt_name AS data_type,
                          c.is_nullable,
                          CASE
                            WHEN EXISTS (
                              SELECT 1
                              FROM information_schema.table_constraints tc
                              JOIN information_schema.key_column_usage kcu
                                ON kcu.constraint_schema = tc.constraint_schema
                               AND kcu.constraint_name = tc.constraint_name
                               AND kcu.table_schema = tc.table_schema
                               AND kcu.table_name = tc.table_name
                              WHERE tc.constraint_type = 'PRIMARY KEY'
                                AND tc.table_schema = c.table_schema
                                AND tc.table_name = c.table_name
                                AND kcu.column_name = c.column_name
                            ) THEN 'PRI'
                            WHEN EXISTS (
                              SELECT 1
                              FROM information_schema.table_constraints tc
                              JOIN information_schema.key_column_usage kcu
                                ON kcu.constraint_schema = tc.constraint_schema
                               AND kcu.constraint_name = tc.constraint_name
                               AND kcu.table_schema = tc.table_schema
                               AND kcu.table_name = tc.table_name
                              WHERE tc.constraint_type = 'UNIQUE'
                                AND tc.table_schema = c.table_schema
                                AND tc.table_name = c.table_name
                                AND kcu.column_name = c.column_name
                            ) THEN 'UNI'
                            ELSE ''
                          END AS column_key,
                          CASE
                            WHEN c.is_identity = 'YES'
                              OR c.column_default LIKE 'nextval(%'
                            THEN 'auto_increment'
                            ELSE ''
                          END AS extra,
                          pg_catalog.col_description(
                            (quote_ident(c.table_schema) || '.' || quote_ident(c.table_name))::regclass::oid,
                            c.ordinal_position
                          ) AS column_comment
                   FROM information_schema.columns c
                   WHERE c.table_schema = current_schema() AND c.table_name = $1
                   ORDER BY c.ordinal_position"#,
            )
            .await
        }
        DatabaseBackend::Sqlite => query_sqlite_columns(db, table_name).await,
        _ => Err(unsupported_backend(backend)),
    }
}

async fn query_columns_with_sql(
    db: &DatabaseConnection,
    table_name: &str,
    sql: &str,
) -> AppResult<Vec<ColumnRow>> {
    ColumnRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|error| AppError::Database(format!("查询表结构失败: {error}")))
}

#[derive(Debug, FromQueryResult)]
struct SqliteColumnRow {
    column_name: String,
    data_type: String,
    not_null: i64,
    primary_key: i64,
}

#[derive(Debug, FromQueryResult)]
struct SqliteIndexRow {
    index_name: String,
    is_unique: i64,
}

#[derive(Debug, FromQueryResult)]
struct SqliteIndexColumnRow {
    column_name: String,
}

#[derive(Debug, FromQueryResult)]
struct SqliteTableSqlRow {
    table_sql: Option<String>,
}

async fn query_sqlite_columns(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<Vec<ColumnRow>> {
    let backend = DatabaseBackend::Sqlite;
    let columns = SqliteColumnRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        r#"SELECT name AS column_name,
                  type AS data_type,
                  "notnull" AS not_null,
                  pk AS primary_key
           FROM pragma_table_info(?)
           ORDER BY cid"#,
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|error| AppError::Database(format!("查询 SQLite 表结构失败: {error}")))?;

    let indexes = SqliteIndexRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        r#"SELECT name AS index_name, "unique" AS is_unique
           FROM pragma_index_list(?)"#,
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|error| AppError::Database(format!("查询 SQLite 索引失败: {error}")))?;
    let mut unique_columns = HashSet::new();
    for index in indexes.into_iter().filter(|index| index.is_unique == 1) {
        let indexed_columns =
            SqliteIndexColumnRow::find_by_statement(Statement::from_sql_and_values(
                backend,
                "SELECT name AS column_name FROM pragma_index_info(?) ORDER BY seqno",
                [index.index_name.into()],
            ))
            .all(db)
            .await
            .map_err(|error| AppError::Database(format!("查询 SQLite 索引列失败: {error}")))?;
        if let [column] = indexed_columns.as_slice() {
            unique_columns.insert(column.column_name.clone());
        }
    }

    let table_sql = SqliteTableSqlRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        "SELECT sql AS table_sql FROM sqlite_master WHERE type = 'table' AND name = ?",
        [table_name.into()],
    ))
    .one(db)
    .await
    .map_err(|error| AppError::Database(format!("查询 SQLite 建表语句失败: {error}")))?
    .and_then(|row| row.table_sql)
    .unwrap_or_default();
    let has_auto_increment = table_sql.to_ascii_uppercase().contains("AUTOINCREMENT");

    Ok(columns
        .into_iter()
        .map(|column| {
            let is_primary = column.primary_key > 0;
            let data_type = normalize_sqlite_type(&column.data_type);
            ColumnRow {
                column_name: column.column_name.clone(),
                data_type,
                is_nullable: if column.not_null == 1 || is_primary {
                    "NO".into()
                } else {
                    "YES".into()
                },
                column_key: if is_primary {
                    "PRI".into()
                } else if unique_columns.contains(&column.column_name) {
                    "UNI".into()
                } else {
                    String::new()
                },
                extra: if is_primary && has_auto_increment {
                    "auto_increment".into()
                } else {
                    String::new()
                },
                column_comment: None,
            }
        })
        .collect())
}

fn normalize_sqlite_type(declared_type: &str) -> String {
    let upper = declared_type.trim().to_ascii_uppercase();
    if upper.contains("BIGINT") {
        "bigint"
    } else if upper.contains("INT") {
        // SQLite stores every INTEGER-affinity value as a signed 64-bit integer.
        "bigint"
    } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
        "text"
    } else if upper.contains("BLOB") || upper.is_empty() {
        "blob"
    } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
        "double"
    } else if upper.contains("DECIMAL") || upper.contains("NUMERIC") {
        "decimal"
    } else if upper.contains("BOOL") {
        "boolean"
    } else if upper.contains("DATE") || upper.contains("TIME") {
        "datetime"
    } else if upper.contains("JSON") {
        "json"
    } else {
        "text"
    }
    .into()
}

#[derive(Debug, FromQueryResult)]
struct TableCommentRow {
    comment: Option<String>,
}

async fn query_table_comment(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<Option<String>> {
    let backend = db.get_database_backend();
    let sql = match backend {
        DatabaseBackend::MySql => {
            "SELECT NULLIF(TABLE_COMMENT, '') AS comment \
             FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = ?"
        }
        DatabaseBackend::Postgres => {
            "SELECT d.description AS comment \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_catalog.pg_description d ON d.objoid = c.oid AND d.objsubid = 0 \
             WHERE n.nspname = current_schema() AND c.relname = $1"
        }
        DatabaseBackend::Sqlite => return Ok(None),
        _ => return Err(unsupported_backend(backend)),
    };
    let result = TableCommentRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        sql,
        [table_name.into()],
    ))
    .one(db)
    .await
    .map_err(|error| AppError::Database(format!("查询表注释失败: {error}")))?;
    Ok(result.and_then(|r| r.comment))
}

#[derive(Debug, FromQueryResult)]
struct TableRow {
    table_name: String,
}

async fn query_tables(db: &DatabaseConnection) -> AppResult<Vec<TableRow>> {
    let backend = db.get_database_backend();
    let sql = match backend {
        DatabaseBackend::MySql => {
            "SELECT TABLE_NAME AS table_name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' \
             AND TABLE_NAME <> 'seaql_migrations' ORDER BY TABLE_NAME"
        }
        DatabaseBackend::Postgres => {
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' \
             AND table_name <> 'seaql_migrations' ORDER BY table_name"
        }
        DatabaseBackend::Sqlite => {
            "SELECT name AS table_name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             AND name <> 'seaql_migrations' ORDER BY name"
        }
        _ => return Err(unsupported_backend(backend)),
    };
    let results = TableRow::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
        .all(db)
        .await
        .map_err(|error| AppError::Database(format!("查询表列表失败: {error}")))?;
    Ok(results)
}

fn unsupported_backend(backend: DatabaseBackend) -> AppError {
    AppError::Validation(format!("代码生成器不支持数据库后端: {backend:?}"))
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database};

    use super::*;

    #[tokio::test]
    async fn sqlite_introspection_preserves_generator_contract() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE seaql_migrations (version TEXT PRIMARY KEY); \
             CREATE TABLE sys_widget ( \
               id INTEGER PRIMARY KEY AUTOINCREMENT, \
               tenant_id TEXT NOT NULL, \
               name TEXT NOT NULL UNIQUE, \
               status TEXT \
             );",
        )
        .await
        .unwrap();

        assert_eq!(list_tables(&db).await.unwrap(), vec!["sys_widget"]);

        let table = fetch_table(&db, "sys_widget").await.unwrap();
        assert_eq!(table.table_name, "sys_widget");
        assert!(table.comment.is_none());
        assert_eq!(table.columns.len(), 4);

        let id = table
            .columns
            .iter()
            .find(|column| column.name == "id")
            .unwrap();
        assert_eq!(id.rust_type, "i64");
        assert!(id.is_primary_key);
        assert!(id.is_auto_increment);
        assert!(!id.is_nullable);

        let name = table
            .columns
            .iter()
            .find(|column| column.name == "name")
            .unwrap();
        assert!(name.is_unique);
        assert!(!name.is_nullable);
    }
}
