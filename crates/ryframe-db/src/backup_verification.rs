//! 显式连接上的只读完整表校验；不发现服务器上的其他数据库。
//!
//! 这是供工作区内租户数据库适配层使用的基础设施接口，不属于 RyFrame 的公共产品 API。

use crate::DbResultExt;
use ryframe_application::ports::backup::{BackupTableDigest, backup_content_hash};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use std::collections::BTreeSet;

const DIGEST_VERSION: &str = "ryframe-table-digest-v1";
const DIGEST_PAGE_VERSION: &str = "ryframe-table-digest-page-v1";

#[derive(FromQueryResult)]
struct PhysicalIdentity {
    server_uuid: String,
    database_name: String,
}

#[derive(FromQueryResult)]
struct Column {
    column_name: String,
    ordinal_position: i64,
}

#[derive(FromQueryResult)]
struct RowDigest {
    row_data: String,
}

#[derive(FromQueryResult)]
struct TableIdentity {
    table_name: String,
    table_type: String,
}

/// 只检查当前连接的全表目录，不忽略额外表或视图。
/// 与数据摘要组合使用时，调用方应传入覆盖两者的同一个只读一致性事务。
pub async fn verify_table_set<C: ConnectionTrait>(db: &C, expected: &[String]) -> AppResult<()> {
    let actual = TableIdentity::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT TABLE_NAME AS table_name, TABLE_TYPE AS table_type FROM information_schema.TABLES \
         WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME",
    ))
    .all(db)
    .await
    .db()?;
    check_table_set(expected, &actual)
}

fn check_table_set(expected: &[String], actual: &[TableIdentity]) -> AppResult<()> {
    let expected_set = expected.iter().collect::<BTreeSet<_>>();
    let actual_set = actual
        .iter()
        .map(|row| &row.table_name)
        .collect::<BTreeSet<_>>();
    if expected.is_empty()
        || expected_set.len() != expected.len()
        || actual_set.len() != actual.len()
        || expected_set != actual_set
        || actual.iter().any(|row| row.table_type != "BASE TABLE")
    {
        return Err(AppError::Validation(
            "目标完整表目录不匹配或包含非基表".into(),
        ));
    }
    Ok(())
}

pub async fn physical_identity<C: ConnectionTrait>(db: &C) -> AppResult<(String, String)> {
    let row = PhysicalIdentity::find_by_statement(Statement::from_string(
        DbBackend::MySql,
        "SELECT @@server_uuid AS server_uuid, DATABASE() AS database_name",
    ))
    .one(db)
    .await
    .db()?
    .ok_or_else(|| AppError::Database("数据库物理身份不可读".into()))?;
    Ok((row.server_uuid, row.database_name))
}

pub fn control_tables() -> AppResult<Vec<String>> {
    Ok(control_table_names()?
        .into_iter()
        .filter(|table| include_table(table))
        .collect())
}

pub fn preserved_control_tables() -> AppResult<Vec<String>> {
    let mut tables = control_table_names()?
        .into_iter()
        .filter(|table| !include_table(table))
        .collect::<Vec<_>>();
    tables.push(crate::migration::CONTROL_MIGRATION_LEDGER.into());
    Ok(tables)
}

fn control_table_names() -> AppResult<Vec<String>> {
    crate::migration::control_ddl_statements()
        .map(|ddl| {
            ddl.split('`')
                .nth(1)
                .map(str::to_owned)
                .ok_or_else(|| AppError::Internal("控制库 DDL 缺少表名".into()))
        })
        .collect()
}

#[derive(FromQueryResult)]
struct PlacementRow {
    tenant_id: String,
    generation: i64,
    switch_token: String,
}

pub async fn placements<C: ConnectionTrait>(
    db: &C,
    target: &str,
) -> AppResult<Vec<ryframe_application::ports::backup::BackupPlacement>> {
    let rows = PlacementRow::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT tenant_id, placement_generation AS generation, switch_token FROM sys_tenant_data_placement \
         WHERE current_target_key = ? ORDER BY tenant_id", [target.into()],
    )).all(db).await.db()?;
    Ok(rows
        .into_iter()
        .map(|row| ryframe_application::ports::backup::BackupPlacement {
            tenant_id: row.tenant_id,
            generation: row.generation,
            switch_token: row.switch_token,
        })
        .collect())
}

fn include_table(table: &str) -> bool {
    !matches!(
        table,
        "ryframe_resource_ownership" | "sys_backup_set" | "sys_backup_resource" | "sys_restore_run"
    )
}

/// 按规范表名顺序计算完整数据摘要。
///
/// 本函数不自行创建事务，以便复用调用方已有的跨表只读事务；调用方必须保证整个调用
/// 使用同一个一致性快照。
pub async fn tables<C: ConnectionTrait>(
    db: &C,
    tables: &[String],
) -> AppResult<Vec<BackupTableDigest>> {
    let unique = unique_table_names(tables)?;
    let mut results = Vec::with_capacity(tables.len());
    for table in unique {
        results.push(table_digest(db, table).await?);
    }
    Ok(results)
}

fn unique_table_names(tables: &[String]) -> AppResult<BTreeSet<&str>> {
    let unique = tables.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if tables.is_empty() || unique.len() != tables.len() {
        return Err(AppError::Validation("数据表清单为空或重复".into()));
    }
    Ok(unique)
}

async fn table_digest<C: ConnectionTrait>(db: &C, table: &str) -> AppResult<BackupTableDigest> {
    let quoted = quote_identifier(table)?;
    let columns = Column::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
        "SELECT COLUMN_NAME AS column_name, CAST(ORDINAL_POSITION AS SIGNED) AS ordinal_position \
         FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
        [table.into()],
    )).all(db).await.db()?;
    let primary = Column::find_by_statement(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT COLUMN_NAME AS column_name, CAST(SEQ_IN_INDEX AS SIGNED) AS ordinal_position \
         FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? \
         AND INDEX_NAME = 'PRIMARY' ORDER BY SEQ_IN_INDEX",
        [table.into()],
    ))
    .all(db)
    .await
    .db()?;
    if columns.is_empty() || primary.is_empty() {
        return Err(AppError::Validation(
            "完整备份校验要求数据表和确定性主键".into(),
        ));
    }
    let expression = columns
        .iter()
        .map(|column| {
            Ok(format!(
                "IFNULL(HEX(CAST({} AS BINARY)), 'NULL')",
                quote_identifier(&column.column_name)?
            ))
        })
        .collect::<AppResult<Vec<_>>>()?
        .join(", ");
    let ordering = primary
        .iter()
        .map(|column| quote_identifier(&column.column_name))
        .collect::<AppResult<Vec<_>>>()?
        .join(", ");
    let mut page_hashes = Vec::new();
    let mut rows = 0_u64;
    loop {
        let page = RowDigest::find_by_statement(Statement::from_sql_and_values(DbBackend::MySql,
            format!("SELECT CONCAT_WS(':', {expression}) AS row_data FROM {quoted} ORDER BY {ordering} LIMIT 500 OFFSET ?"),
            [rows.into()],
        )).all(db).await.db()?;
        let count = page.len();
        if count > 0 {
            let page_rows = page.into_iter().map(|row| row.row_data).collect::<Vec<_>>();
            page_hashes.push(page_content_hash(rows, &page_rows)?);
        }
        rows += count as u64;
        if count < 500 {
            break;
        }
    }
    Ok(BackupTableDigest {
        table: table.into(),
        rows,
        sha256: table_content_hash(table, &columns, rows, &page_hashes)?,
    })
}

fn page_content_hash(offset: u64, rows: &[String]) -> AppResult<String> {
    backup_content_hash(&(DIGEST_PAGE_VERSION, offset, rows))
}

fn table_content_hash(
    table: &str,
    columns: &[Column],
    rows: u64,
    page_hashes: &[String],
) -> AppResult<String> {
    let column_contract = columns
        .iter()
        .map(|column| (column.ordinal_position, column.column_name.as_str()))
        .collect::<Vec<_>>();
    backup_content_hash(&(DIGEST_VERSION, table, column_contract, rows, page_hashes))
}

fn quote_identifier(value: &str) -> AppResult<String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(AppError::Validation("数据表或列名不安全".into()));
    }
    Ok(format!("`{value}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_table_set_rejects_missing_extra_duplicate_and_views() {
        let expected = vec!["business".to_owned(), "ledger".to_owned()];
        let row = |name: &str, kind: &str| TableIdentity {
            table_name: name.into(),
            table_type: kind.into(),
        };
        assert!(
            check_table_set(
                &expected,
                &[row("ledger", "BASE TABLE"), row("business", "BASE TABLE")]
            )
            .is_ok()
        );
        for actual in [
            vec![row("business", "BASE TABLE")],
            vec![
                row("ledger", "BASE TABLE"),
                row("business", "BASE TABLE"),
                row("extra", "BASE TABLE"),
            ],
            vec![row("business", "BASE TABLE"), row("ledger", "VIEW")],
            vec![row("business", "BASE TABLE"), row("business", "BASE TABLE")],
        ] {
            assert!(check_table_set(&expected, &actual).is_err());
        }
        assert!(check_table_set(&[], &[]).is_err());
        assert!(
            check_table_set(
                &["business".into(), "business".into()],
                &[row("business", "BASE TABLE")]
            )
            .is_err()
        );
    }

    #[test]
    fn only_declared_tables_and_safe_identifiers_are_accepted() {
        let tables = control_tables().unwrap();
        assert!(tables.contains(&"sys_user".to_owned()));
        assert!(tables.contains(&"sys_tenant_data_placement".to_owned()));
        assert!(!tables.contains(&"sys_backup_set".to_owned()));
        assert!(!tables.contains(&"ryframe_resource_ownership".to_owned()));
        for value in ["", "sys_user;DELETE", "other.table", "`sys_user`"] {
            assert!(quote_identifier(value).is_err());
        }
    }

    #[tokio::test]
    async fn table_digest_list_rejects_empty_and_duplicate_input_before_querying() {
        let database = sea_orm::DatabaseConnection::default();
        for input in [vec![], vec!["business".into(), "business".into()]] {
            let error = tables(&database, &input).await.unwrap_err();
            assert!(matches!(
                error,
                AppError::Validation(message) if message == "数据表清单为空或重复"
            ));
        }
        assert_eq!(
            unique_table_names(&["ledger".into(), "business".into()])
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["business", "ledger"]
        );
    }

    #[test]
    fn digest_canonical_input_binds_order_and_page_boundaries() {
        let columns = vec![
            Column {
                column_name: "id".into(),
                ordinal_position: 1,
            },
            Column {
                column_name: "payload".into(),
                ordinal_position: 2,
            },
        ];
        let page = |offset, rows: &[&str]| {
            page_content_hash(
                offset,
                &rows.iter().map(|row| (*row).into()).collect::<Vec<_>>(),
            )
            .unwrap()
        };
        let ordered = vec![page(0, &["01:41", "02:42"]), page(2, &["03:43"])];
        let same = vec![page(0, &["01:41", "02:42"]), page(2, &["03:43"])];
        let reordered = vec![page(0, &["02:42", "01:41"]), page(2, &["03:43"])];
        let other_boundary = vec![page(0, &["01:41"]), page(1, &["02:42", "03:43"])];
        let digest = table_content_hash("business", &columns, 3, &ordered).unwrap();
        let reversed_columns = vec![
            Column {
                column_name: "payload".into(),
                ordinal_position: 1,
            },
            Column {
                column_name: "id".into(),
                ordinal_position: 2,
            },
        ];
        assert_eq!(
            digest,
            table_content_hash("business", &columns, 3, &same).unwrap()
        );
        assert_ne!(
            digest,
            table_content_hash("business", &columns, 3, &reordered).unwrap()
        );
        assert_ne!(
            digest,
            table_content_hash("business", &columns, 3, &other_boundary).unwrap()
        );
        assert_ne!(
            digest,
            table_content_hash("other", &columns, 3, &ordered).unwrap()
        );
        assert_ne!(
            digest,
            table_content_hash("business", &reversed_columns, 3, &ordered).unwrap()
        );
        assert_ne!(
            digest,
            table_content_hash("business", &columns, 4, &ordered).unwrap()
        );
    }
}
