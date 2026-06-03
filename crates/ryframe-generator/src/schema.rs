use ryframe_common::AppResult;
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone, Serialize)]
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

    Ok(TableInfo {
        table_name: table_name.to_string(),
        comment: None,
        columns: col_infos,
    })
}

/// 列出数据库中所有表
pub async fn list_tables(db: &DatabaseConnection) -> AppResult<Vec<String>> {
    let tables = query_tables(db).await?;
    Ok(tables.into_iter().map(|t| t.table_name).collect())
}

/// 获取主键的 Rust 类型（通用工具函数）
pub fn get_pk_type(table: &TableInfo) -> &'static str {
    for col in &table.columns {
        if col.is_primary_key {
            return if col.rust_type.contains("i64") {
                "i64"
            } else {
                "i32"
            };
        }
    }
    "i64"
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
    let sql = "SELECT COLUMN_NAME as column_name, DATA_TYPE as data_type, IS_NULLABLE as is_nullable, \
         COLUMN_KEY as column_key, EXTRA as extra, COLUMN_COMMENT as column_comment \
         FROM information_schema.columns WHERE table_name = $1 \
         ORDER BY ORDINAL_POSITION";
    let results = ColumnRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|e| ryframe_common::AppError::Database(format!("查询表结构失败: {}", e)))?;
    Ok(results)
}

#[derive(Debug, FromQueryResult)]
struct TableRow {
    table_name: String,
}

async fn query_tables(db: &DatabaseConnection) -> AppResult<Vec<TableRow>> {
    let backend = db.get_database_backend();
    let sql = match backend {
        DatabaseBackend::MySql => {
            "SELECT TABLE_NAME as table_name FROM information_schema.tables WHERE table_schema = DATABASE()"
        }
        DatabaseBackend::Postgres => {
            "SELECT TABLE_NAME as table_name FROM information_schema.tables WHERE table_schema NOT IN ('information_schema', 'pg_catalog')"
        }
        DatabaseBackend::Sqlite => {
            "SELECT name as table_name FROM sqlite_master WHERE type = 'table'"
        }
        _ => {
            "SELECT TABLE_NAME as table_name FROM information_schema.tables WHERE table_schema = DATABASE()"
        }
    };
    let results = TableRow::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
        .all(db)
        .await
        .map_err(|e| ryframe_common::AppError::Database(format!("查询表列表失败: {}", e)))?;
    Ok(results)
}
