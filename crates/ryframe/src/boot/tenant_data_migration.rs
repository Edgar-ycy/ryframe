use std::sync::Arc;

use ryframe_application::{
    TenantDataCatalogTable, TenantDataCleanupOwnership as ApplicationCleanupOwnership,
    TenantDataFence, TenantDataMigrationFuture, TenantDataMigrationPort, TenantDataRow,
    TenantDataRowBatch,
};
use ryframe_kernel::{AppError, AppResult};
use ryframe_tenant_db::{
    TenantDataCleanupBatch, TenantDataCleanupOwnership, TenantDatabaseRouter,
    migration::{TENANT_DATA_CATALOG, TenantDataTableDescriptor},
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};

use super::tenant_data::map_error as map_tenant_data_error;

struct TenantDataMigrationBridge {
    router: Arc<TenantDatabaseRouter>,
}

impl TenantDataMigrationPort for TenantDataMigrationBridge {
    fn catalog_tables(&self) -> Vec<TenantDataCatalogTable> {
        TENANT_DATA_CATALOG
            .tables()
            .iter()
            .map(|table| TenantDataCatalogTable {
                name: table.table,
                copy_order: table.copy_order,
            })
            .collect()
    }

    fn prepare_target<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .prepare_migration_target_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn clear_prepared_target<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .clear_prepared_target_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn freeze_fence<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .freeze_fence_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn activate_fence<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .activate_fence_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn assert_frozen_fence<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .assert_frozen_fence_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn cleanup_ownership<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a, ApplicationCleanupOwnership> {
        Box::pin(async move {
            self.router
                .cleanup_ownership_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map(map_cleanup_ownership)
                .map_err(map_tenant_data_error)
        })
    }

    fn delete_rows_batch<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
        table: &'a str,
        batch_size: u32,
    ) -> TenantDataMigrationFuture<'a, u64> {
        Box::pin(async move {
            let descriptor = catalog_table(table)?;
            self.router
                .delete_tenant_rows_batch_for_catalog(
                    TenantDataCleanupBatch {
                        tenant_id: fence.tenant_id,
                        target_key: fence.target_key,
                        placement_generation: fence.generation,
                        switch_token: fence.switch_token,
                        descriptor,
                        batch_size,
                    },
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn finish_cleanup<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .finish_tenant_cleanup_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn read_rows_batch<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: &'a str,
        table: &'a str,
        cursor: Option<&'a [String]>,
        batch_size: u32,
    ) -> TenantDataMigrationFuture<'a, TenantDataRowBatch> {
        Box::pin(async move {
            validate_batch_size(batch_size)?;
            let descriptor = catalog_table(table)?;
            let target = self
                .router
                .open_target_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_tenant_data_error)?;
            let business_cursor = business_cursor_columns(descriptor);
            if cursor.is_some_and(|values| values.len() != business_cursor.len()) {
                return Err(AppError::Conflict("迁移 cursor 与 catalog 不兼容".into()));
            }
            let select_columns = descriptor
                .checksum_columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    format!(
                        "IF(`{column}` IS NULL, NULL, TO_BASE64(CAST(`{column}` AS BINARY))) AS `c{index}`"
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let mut sql = format!(
                "SELECT {select_columns} FROM `{}` WHERE `{}` = ?",
                descriptor.table, descriptor.tenant_column
            );
            let mut values: Vec<sea_orm::Value> = vec![tenant_id.into()];
            if let Some(cursor_values) = cursor {
                let cursor_columns = binary_cursor_columns(&business_cursor);
                let parameters = business_cursor
                    .iter()
                    .map(|_| "FROM_BASE64(?)")
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!(" AND ({cursor_columns}) > ({parameters})"));
                values.extend(cursor_values.iter().cloned().map(Into::into));
            }
            sql.push_str(&format!(
                " ORDER BY {} LIMIT {batch_size}",
                binary_cursor_columns(&business_cursor)
            ));
            let query_rows = target
                .connection()
                .query_all_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    sql,
                    values,
                ))
                .await
                .map_err(database_error)?;
            let rows = decode_rows(&query_rows, descriptor.checksum_columns.len())?;
            let next_cursor = (!rows.is_empty())
                .then(|| cursor_from_last_row(&rows, descriptor, &business_cursor))
                .transpose()?;
            Ok(TenantDataRowBatch { rows, next_cursor })
        })
    }

    fn write_rows_batch<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
        table: &'a str,
        rows: &'a [TenantDataRow],
    ) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            let descriptor = catalog_table(table)?;
            let target = self
                .router
                .open_target_for_catalog(fence.target_key, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_tenant_data_error)?;
            let transaction = target.connection().begin().await.map_err(database_error)?;
            lock_target_fence(&transaction, fence, target.is_dedicated()).await?;
            let column_list = descriptor
                .checksum_columns
                .iter()
                .map(|column| format!("`{column}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = descriptor
                .checksum_columns
                .iter()
                .map(|_| "FROM_BASE64(?)")
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "INSERT INTO `{}` ({column_list}) VALUES ({placeholders}) ON DUPLICATE KEY UPDATE `tenant_id` = VALUES(`tenant_id`)",
                descriptor.table
            );
            for row in rows {
                transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DbBackend::MySql,
                        sql.clone(),
                        row.iter().cloned().map(Into::into),
                    ))
                    .await
                    .map_err(database_error)?;
            }
            transaction.commit().await.map_err(database_error)
        })
    }

    fn verify_foreign_keys<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: &'a str,
        table: &'a str,
    ) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            let descriptor = catalog_table(table)?;
            let target = self
                .router
                .open_target_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_tenant_data_error)?;
            for foreign_key in descriptor.foreign_keys {
                let join = foreign_key
                    .columns
                    .iter()
                    .zip(foreign_key.referenced_columns)
                    .map(|(column, referenced)| format!("child.`{column}` = parent.`{referenced}`"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let non_null = foreign_key
                    .columns
                    .iter()
                    .map(|column| format!("child.`{column}` IS NOT NULL"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let referenced_tenant = foreign_key
                    .referenced_columns
                    .iter()
                    .find(|column| **column == "tenant_id")
                    .ok_or_else(|| AppError::Config("catalog 外键缺少 tenant_id".into()))?;
                let sql = format!(
                    "SELECT EXISTS(SELECT 1 FROM `{child}` AS child \
                     LEFT JOIN `{parent}` AS parent ON {join} \
                     WHERE child.`tenant_id` = ? AND {non_null} \
                       AND parent.`{referenced_tenant}` IS NULL LIMIT 1)",
                    child = descriptor.table,
                    parent = foreign_key.referenced_table,
                );
                let row = target
                    .connection()
                    .query_one_raw(Statement::from_sql_and_values(
                        DbBackend::MySql,
                        sql,
                        [tenant_id.into()],
                    ))
                    .await
                    .map_err(database_error)?
                    .ok_or_else(|| AppError::Database("租户数据外键校验无结果".into()))?;
                if row.try_get_by_index::<i64>(0).map_err(database_error)? != 0 {
                    return Err(AppError::Conflict(format!(
                        "租户数据外键校验失败: {}.{}",
                        descriptor.table, foreign_key.name
                    )));
                }
            }
            Ok(())
        })
    }
}

pub fn port(router: Arc<TenantDatabaseRouter>) -> Arc<dyn TenantDataMigrationPort> {
    Arc::new(TenantDataMigrationBridge { router })
}

fn catalog_table(table: &str) -> AppResult<&'static TenantDataTableDescriptor> {
    TENANT_DATA_CATALOG
        .tables()
        .iter()
        .find(|descriptor| descriptor.table == table)
        .ok_or_else(|| AppError::Validation(format!("未知租户数据表: {table}")))
}

fn validate_batch_size(batch_size: u32) -> AppResult<()> {
    if (1..=10_000).contains(&batch_size) {
        Ok(())
    } else {
        Err(AppError::Validation(
            "租户数据批次大小必须为 1..=10000".into(),
        ))
    }
}

fn business_cursor_columns(descriptor: &TenantDataTableDescriptor) -> Vec<&'static str> {
    descriptor
        .primary_key_cursor_columns
        .iter()
        .copied()
        .filter(|column| *column != descriptor.tenant_column)
        .collect()
}

fn binary_cursor_columns(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| format!("CAST(`{column}` AS BINARY)"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn decode_rows(
    rows: &[sea_orm::QueryResult],
    column_count: usize,
) -> AppResult<Vec<TenantDataRow>> {
    rows.iter()
        .map(|row| {
            (0..column_count)
                .map(|index| {
                    row.try_get_by_index::<Option<String>>(index)
                        .map_err(database_error)
                })
                .collect::<AppResult<Vec<_>>>()
        })
        .collect()
}

fn cursor_from_last_row(
    rows: &[TenantDataRow],
    descriptor: &TenantDataTableDescriptor,
    cursor_columns: &[&str],
) -> AppResult<Vec<String>> {
    let last = rows
        .last()
        .ok_or_else(|| AppError::Internal("空租户数据批次没有 cursor".into()))?;
    cursor_columns
        .iter()
        .map(|column| {
            let index = descriptor
                .checksum_columns
                .iter()
                .position(|candidate| candidate == column)
                .ok_or_else(|| AppError::Conflict("主键 cursor 不在 checksum 列中".into()))?;
            last[index]
                .clone()
                .ok_or_else(|| AppError::Conflict("主键 cursor 不能为 NULL".into()))
        })
        .collect()
}

async fn lock_target_fence(
    transaction: &DatabaseTransaction,
    fence: TenantDataFence<'_>,
    dedicated: bool,
) -> AppResult<()> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT target_key, placement_generation, state, switch_token FROM biz_tenant_fence WHERE tenant_id = ? FOR UPDATE",
            [fence.tenant_id.into()],
        ))
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::TenantDataMaintenance("迁移目标 fence 不存在".into(), 5))?;
    let target_key: String = row.try_get("", "target_key").map_err(database_error)?;
    let generation: i64 = row
        .try_get("", "placement_generation")
        .map_err(database_error)?;
    let state: String = row.try_get("", "state").map_err(database_error)?;
    let token: String = row.try_get("", "switch_token").map_err(database_error)?;
    if target_key != fence.target_key
        || generation != fence.generation
        || state != "frozen"
        || token != fence.switch_token
    {
        return Err(AppError::StalePlacementGeneration(
            "迁移目标 fence 已变化".into(),
        ));
    }
    if dedicated {
        let slot = transaction
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                "SELECT tenant_id, placement_generation, switch_token FROM biz_tenant_target_slot WHERE slot_id = 1 FOR UPDATE",
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| {
                AppError::TenantDataTargetUnavailable("专属目标槽不存在".into(), 5)
            })?;
        let tenant_id: Option<String> = slot.try_get("", "tenant_id").map_err(database_error)?;
        let slot_generation: Option<i64> = slot
            .try_get("", "placement_generation")
            .map_err(database_error)?;
        let slot_token: Option<String> =
            slot.try_get("", "switch_token").map_err(database_error)?;
        if tenant_id.as_deref() != Some(fence.tenant_id)
            || slot_generation != Some(fence.generation)
            || slot_token.as_deref() != Some(fence.switch_token)
        {
            return Err(AppError::TenantOperationConflict("专属目标槽已变化".into()));
        }
    }
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

const fn map_cleanup_ownership(
    ownership: TenantDataCleanupOwnership,
) -> ApplicationCleanupOwnership {
    match ownership {
        TenantDataCleanupOwnership::OwnedFrozen => ApplicationCleanupOwnership::OwnedFrozen,
        TenantDataCleanupOwnership::AlreadyClean => ApplicationCleanupOwnership::AlreadyClean,
        TenantDataCleanupOwnership::NotOwned => ApplicationCleanupOwnership::NotOwned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_ownership_mapping_is_complete() {
        assert_eq!(
            map_cleanup_ownership(TenantDataCleanupOwnership::OwnedFrozen),
            ApplicationCleanupOwnership::OwnedFrozen
        );
        assert_eq!(
            map_cleanup_ownership(TenantDataCleanupOwnership::AlreadyClean),
            ApplicationCleanupOwnership::AlreadyClean
        );
        assert_eq!(
            map_cleanup_ownership(TenantDataCleanupOwnership::NotOwned),
            ApplicationCleanupOwnership::NotOwned
        );
    }

    #[test]
    fn catalog_lookup_rejects_unknown_table() {
        assert!(catalog_table("unknown_table").is_err());
    }

    #[test]
    fn batch_size_is_bounded() {
        assert!(validate_batch_size(1).is_ok());
        assert!(validate_batch_size(10_000).is_ok());
        assert!(validate_batch_size(0).is_err());
        assert!(validate_batch_size(10_001).is_err());
    }

    #[test]
    fn cursor_is_derived_from_catalog_columns() {
        const COLUMNS: &[&str] = &["tenant_id", "id", "name"];
        const CURSOR_COLUMNS: &[&str] = &["tenant_id", "id"];
        const COLUMN_TYPES: &[&str] = &["varchar", "bigint", "varchar"];
        const DESCRIPTOR: TenantDataTableDescriptor = TenantDataTableDescriptor {
            table: "biz_example",
            copy_order: 1,
            tenant_column: "tenant_id",
            primary_key_cursor_columns: CURSOR_COLUMNS,
            checksum_columns: COLUMNS,
            column_types: COLUMN_TYPES,
            has_generated_columns: false,
            foreign_key_dependencies: &[],
            foreign_keys: &[],
            schema_canonical: "test",
        };

        let cursor_columns = business_cursor_columns(&DESCRIPTOR);
        let row = DESCRIPTOR
            .checksum_columns
            .iter()
            .map(|column| Some((*column).to_owned()))
            .collect::<TenantDataRow>();
        let cursor = cursor_from_last_row(&[row], &DESCRIPTOR, &cursor_columns)
            .expect("应按 catalog 生成 cursor");
        assert_eq!(
            cursor,
            cursor_columns
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<Vec<_>>()
        );
    }
}
