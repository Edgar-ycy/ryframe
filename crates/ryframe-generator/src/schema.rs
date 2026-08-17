use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub table_name: String,
    pub comment: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub foreign_key_dependencies: Vec<String>,
    pub schema_canonical: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub index_type: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
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
        return Err(AppError::Validation(
            "表名只能包含字母、数字和下划线".into(),
        ));
    }

    let columns = query_columns(db, table_name).await?;

    let mut col_infos: Vec<ColumnInfo> = Vec::new();
    for col in columns {
        if col.extra.to_ascii_lowercase().contains("generated") {
            return Err(AppError::Validation(format!(
                "租户业务表 {table_name} 的生成列 {} 暂不支持复制，请改为普通持久列",
                col.column_name
            )));
        }
        if col.data_type.eq_ignore_ascii_case("timestamp") {
            return Err(AppError::Validation(format!(
                "租户业务表 {table_name} 的 TIMESTAMP 列 {} 暂不支持跨目标复制，请使用 DATETIME(6)",
                col.column_name
            )));
        }
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

    ryframe_tenant_db_migration::verify_mysql_84(db)
        .await
        .map_err(|_| AppError::Validation("代码生成要求 Oracle MySQL 8.4.x".into()))?;
    let table_comment = query_table_comment(db, table_name).await?;
    let indexes = query_indexes(db, table_name).await?;
    let foreign_keys = query_foreign_keys(db, table_name).await?;
    let mut foreign_key_dependencies = foreign_keys
        .iter()
        .map(|foreign_key| foreign_key.referenced_table.clone())
        .collect::<Vec<_>>();
    foreign_key_dependencies.sort_unstable();
    foreign_key_dependencies.dedup();
    let schema_canonical = ryframe_tenant_db_migration::canonical_table_schema(db, table_name)
        .await
        .map_err(|error| AppError::Database(format!("规范化业务表结构失败: {error}")))?;

    Ok(TableInfo {
        table_name: table_name.to_string(),
        comment: table_comment,
        columns: col_infos,
        indexes,
        foreign_keys,
        foreign_key_dependencies,
        schema_canonical,
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
        .find(|column| column.is_primary_key && column.name != "tenant_id")
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
        r#"SELECT COLUMN_NAME AS `column_name`,
                          DATA_TYPE AS `data_type`,
                          IS_NULLABLE AS `is_nullable`,
                          COLUMN_KEY AS `column_key`,
                          EXTRA AS `extra`,
                          NULLIF(COLUMN_COMMENT, '') AS `column_comment`
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
    let sql = "SELECT NULLIF(TABLE_COMMENT, '') AS `comment` \
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
struct IndexColumnRow {
    index_name: String,
    non_unique: i64,
    index_type: String,
    column_name: String,
}

async fn query_indexes(db: &DatabaseConnection, table_name: &str) -> AppResult<Vec<IndexInfo>> {
    let rows = IndexColumnRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT index_name AS `index_name`, CAST(non_unique AS SIGNED) AS `non_unique`, \
         index_type AS `index_type`, column_name AS `column_name` \
         FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = ? \
         ORDER BY index_name, seq_in_index",
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|error| AppError::Database(format!("查询索引结构失败: {error}")))?;
    let mut indexes: Vec<IndexInfo> = Vec::new();
    for row in rows {
        if let Some(index) = indexes.last_mut()
            && index.name == row.index_name
        {
            index.columns.push(row.column_name);
            continue;
        }
        indexes.push(IndexInfo {
            name: row.index_name,
            unique: row.non_unique == 0,
            index_type: row.index_type,
            columns: vec![row.column_name],
        });
    }
    Ok(indexes)
}

#[derive(Debug, FromQueryResult)]
struct ForeignKeyColumnRow {
    constraint_name: String,
    column_name: String,
    referenced_table_schema: String,
    current_schema: String,
    referenced_table_name: String,
    referenced_column_name: String,
}

async fn query_foreign_keys(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<Vec<ForeignKeyInfo>> {
    let rows = ForeignKeyColumnRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT constraint_name AS `constraint_name`, column_name AS `column_name`, \
         referenced_table_schema AS `referenced_table_schema`, \
         DATABASE() AS `current_schema`, \
         referenced_table_name AS `referenced_table_name`, \
         referenced_column_name AS `referenced_column_name` \
         FROM information_schema.key_column_usage \
         WHERE table_schema = DATABASE() AND table_name = ? \
           AND referenced_table_name IS NOT NULL \
         ORDER BY constraint_name, ordinal_position",
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|error| AppError::Database(format!("查询外键结构失败: {error}")))?;
    let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
    for row in rows {
        if row.referenced_table_schema != row.current_schema {
            return Err(AppError::Validation(format!(
                "租户业务表 {table_name} 的外键 {} 跨 schema 引用 {}，已拒绝生成",
                row.constraint_name, row.referenced_table_schema
            )));
        }
        if let Some(foreign_key) = foreign_keys.last_mut()
            && foreign_key.name == row.constraint_name
        {
            if foreign_key.referenced_table != row.referenced_table_name {
                return Err(AppError::Validation("外键引用表结构不一致".into()));
            }
            foreign_key.columns.push(row.column_name);
            foreign_key
                .referenced_columns
                .push(row.referenced_column_name);
            continue;
        }
        foreign_keys.push(ForeignKeyInfo {
            name: row.constraint_name,
            columns: vec![row.column_name],
            referenced_table: row.referenced_table_name,
            referenced_columns: vec![row.referenced_column_name],
        });
    }
    Ok(foreign_keys)
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
    let sql = "SELECT TABLE_NAME AS `table_name` FROM information_schema.tables \
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
