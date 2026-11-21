use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};

use super::model::{ColumnInfo, IndexInfo, TableInfo};

mod foreign_keys;
use foreign_keys::query_foreign_keys;

/// 只读取既有 MySQL 表结构，供调用方生成并人工确认资源清单草案。
///
/// 本入口不生成产品代码、不写文件，也不推断页面布局或业务事务。
pub async fn inspect_existing_table(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<TableInfo> {
    // 验证表名只包含字母、数字和下划线，防止注入
    if table_name.is_empty()
        || !table_name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(AppError::Validation(
            "表名只能包含字母、数字和下划线".into(),
        ));
    }

    ryframe_tenant_db::migration::verify_mysql_80(db)
        .await
        .map_err(|error| {
            AppError::Validation(format!("表结构导入要求 MySQL 8.0.16 或更高版本：{error}"))
        })?;
    if !query_tables(db)
        .await?
        .iter()
        .any(|table| table.table_name == table_name)
    {
        return Err(AppError::Validation(format!(
            "未找到可导入的普通表 {table_name}；请确认来源数据库、表名及查询权限，视图和迁移账本不能导入"
        )));
    }
    let columns = query_columns(db, table_name).await?;
    if columns.is_empty() {
        return Err(AppError::Validation(format!(
            "表 {table_name} 没有可读取的列；检查 information_schema 权限"
        )));
    }

    let mut col_infos: Vec<ColumnInfo> = Vec::new();
    for col in columns {
        let extra = col.extra.to_ascii_lowercase();
        if extra.contains("virtual generated") || extra.contains("stored generated") {
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
        let rust_type =
            super::type_mapping::mysql_rust_type(&col.column_type, col.is_nullable == "YES")
                .map_err(|error| {
                    AppError::Validation(format!(
                        "表 {table_name}，字段 {}：{error}",
                        col.column_name
                    ))
                })?;
        let col_info = ColumnInfo {
            name: col.column_name.clone(),
            data_type: col.data_type.clone(),
            column_type: col.column_type,
            collation: col.collation,
            default: col.column_default,
            extra: col.extra.clone(),
            rust_type,
            is_nullable: col.is_nullable == "YES",
            is_primary_key: col.column_key == "PRI",
            is_unique: col.column_key == "UNI",
            is_auto_increment: col.extra.contains("auto_increment"),
            comment: col.column_comment,
        };
        col_infos.push(col_info);
    }

    let table_options = query_table_options(db, table_name).await?;
    let indexes = query_indexes(db, table_name).await?;
    let foreign_keys = query_foreign_keys(db, table_name).await?;
    let mut foreign_key_dependencies = foreign_keys
        .iter()
        .map(|foreign_key| foreign_key.referenced_table.clone())
        .collect::<Vec<_>>();
    foreign_key_dependencies.sort_unstable();
    foreign_key_dependencies.dedup();
    let schema_canonical = ryframe_db::schema::canonical_table_schema(db, table_name)
        .await
        .map_err(|error| AppError::Database(format!("规范化业务表结构失败: {error}")))?;

    Ok(TableInfo {
        table_name: table_name.to_string(),
        comment: table_options.comment,
        collation: Some(table_options.collation),
        columns: col_infos,
        indexes,
        foreign_keys,
        foreign_key_dependencies,
        schema_canonical,
    })
}

/// 列出可供显式导入的既有 MySQL 表；结果不触发任何写入。
pub async fn list_existing_tables(db: &DatabaseConnection) -> AppResult<Vec<String>> {
    let tables = query_tables(db).await?;
    Ok(tables.into_iter().map(|t| t.table_name).collect())
}

#[derive(Debug, FromQueryResult)]
struct ColumnRow {
    column_name: String,
    data_type: String,
    column_type: String,
    collation: Option<String>,
    column_default: Option<String>,
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
                          COLUMN_TYPE AS `column_type`,
                          COLLATION_NAME AS `collation`,
                          COLUMN_DEFAULT AS `column_default`,
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
struct TableOptionsRow {
    comment: Option<String>,
    collation: String,
}

async fn query_table_options(
    db: &DatabaseConnection,
    table_name: &str,
) -> AppResult<TableOptionsRow> {
    let backend = db.get_database_backend();
    if backend != DatabaseBackend::MySql {
        return Err(unsupported_backend(backend));
    }
    let sql = "SELECT NULLIF(TABLE_COMMENT, '') AS `comment`, TABLE_COLLATION AS `collation` \
             FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = ?";
    TableOptionsRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        sql,
        [table_name.into()],
    ))
    .one(db)
    .await
    .map_err(|error| AppError::Database(format!("查询表选项失败: {error}")))?
    .ok_or_else(|| {
        AppError::Validation(format!(
            "读取表选项时 {table_name} 已不存在；重新核对来源后预览，本批未写入"
        ))
    })
}

#[derive(Debug, FromQueryResult)]
struct IndexColumnRow {
    index_name: String,
    non_unique: i64,
    index_type: String,
    column_name: Option<String>,
    prefix_length: Option<u64>,
    direction: Option<String>,
    visible: String,
}

async fn query_indexes(db: &DatabaseConnection, table_name: &str) -> AppResult<Vec<IndexInfo>> {
    let rows = IndexColumnRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT index_name AS `index_name`, CAST(non_unique AS SIGNED) AS `non_unique`, \
         index_type AS `index_type`, column_name AS `column_name`, \
         CAST(sub_part AS UNSIGNED) AS `prefix_length`, collation AS `direction`, is_visible AS `visible` \
         FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = ? \
         ORDER BY index_name, seq_in_index",
        [table_name.into()],
    ))
    .all(db)
    .await
    .map_err(|error| AppError::Database(format!("查询索引结构失败: {error}")))?;
    let mut indexes: Vec<IndexInfo> = Vec::new();
    for row in rows {
        if row.prefix_length.is_some()
            || row.direction.as_deref() != Some("A")
            || row.visible != "YES"
            || row.column_name.is_none()
            || row.index_type != "BTREE"
        {
            return Err(AppError::Validation(format!(
                "表 {table_name} 的索引 {} 暂不支持标准生成：要求可见、升序、完整列 BTREE 索引；前缀、降序、函数、全文或空间索引须使用明确迁移扩展；本批未写入",
                row.index_name
            )));
        }
        let column = row.column_name.expect("已确认完整列索引");
        if let Some(index) = indexes.last_mut()
            && index.name == row.index_name
        {
            index.columns.push(column);
            continue;
        }
        indexes.push(IndexInfo {
            name: row.index_name,
            unique: row.non_unique == 0,
            index_type: row.index_type,
            columns: vec![column],
        });
    }
    Ok(indexes)
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
