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
    if backend != DatabaseBackend::MySql {
        return Err(unsupported_backend(backend));
    }
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
struct TableCommentRow {
    comment: Option<String>,
}

async fn query_table_comment(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<Option<String>> {
    let backend = db.get_database_backend();
    if backend != DatabaseBackend::MySql {
        return Err(unsupported_backend(backend));
    }
    let sql = "SELECT NULLIF(TABLE_COMMENT, '') AS comment \
             FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = ?";
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
    if backend != DatabaseBackend::MySql {
        return Err(unsupported_backend(backend));
    }
    let sql = "SELECT TABLE_NAME AS table_name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' \
             AND TABLE_NAME <> 'seaql_migrations' ORDER BY TABLE_NAME";
    let results = TableRow::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
        .all(db)
        .await
        .map_err(|error| AppError::Database(format!("查询表列表失败: {error}")))?;
    Ok(results)
}

fn unsupported_backend(backend: DatabaseBackend) -> AppError {
    AppError::Validation(format!("代码生成器不支持数据库后端: {backend:?}"))
}
