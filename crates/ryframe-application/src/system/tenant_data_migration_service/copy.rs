use chrono::Utc;
use ryframe_db::{tenant_data_migration, tenant_data_migration_item};
use ryframe_kernel::{AppError, AppResult};
use ryframe_tenant_db::TenantDataTargetHandle;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};

use super::workflow::ensure_not_cancel_requested;
use super::{TenantDataMigrationService, database_error};

impl TenantDataMigrationService {
    pub(super) async fn copy_catalog(
        &self,
        migration: &tenant_data_migration::Model,
    ) -> AppResult<()> {
        let source = self
            .router
            .open_target_for_catalog(&migration.source_target_key, &self.catalog)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let target = self
            .router
            .open_target_for_catalog(&migration.target_key, &self.catalog)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let existing = self
            .repository
            .items(self.database.write(), migration.id)
            .await?;
        for descriptor in self.catalog.tables() {
            let mut item = if let Some(item) = existing
                .iter()
                .find(|item| item.table_name == descriptor.table)
                .cloned()
            {
                item
            } else {
                let now = Utc::now();
                self.repository
                    .insert_item(
                        self.database.write(),
                        tenant_data_migration_item::Model {
                            id: crate::next_id()?,
                            migration_id: migration.id,
                            table_name: descriptor.table.into(),
                            copy_order: i32::try_from(descriptor.copy_order).map_err(|_| {
                                AppError::Validation("catalog copy_order 超出范围".into())
                            })?,
                            state: tenant_data_migration_item::Model::STATE_PENDING.into(),
                            cursor_json: None,
                            source_row_count: Some(0),
                            target_row_count: Some(0),
                            source_digest: None,
                            target_digest: None,
                            error_code: None,
                            error_detail: None,
                            copy_started_at: None,
                            copied_at: None,
                            verified_at: None,
                            cleanup_state: tenant_data_migration_item::Model::CLEANUP_PENDING
                                .into(),
                            cleanup_row_count: 0,
                            created_at: now,
                            updated_at: now,
                        },
                    )
                    .await?
            };
            if item.state == tenant_data_migration_item::Model::STATE_VERIFIED {
                continue;
            }
            item.state = tenant_data_migration_item::Model::STATE_COPYING.into();
            item.copy_started_at.get_or_insert(Utc::now());
            item.updated_at = Utc::now();
            item = self
                .repository
                .save_item(self.database.write(), item)
                .await?;
            item = self
                .copy_table_in_batches(migration, descriptor, &source, &target, item)
                .await?;
            item.state = tenant_data_migration_item::Model::STATE_COPIED.into();
            item.copied_at = Some(Utc::now());
            item.updated_at = Utc::now();
            self.repository
                .save_item(self.database.write(), item)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn verify_catalog(
        &self,
        migration: &tenant_data_migration::Model,
    ) -> AppResult<()> {
        let source = self
            .router
            .open_target_for_catalog(&migration.source_target_key, &self.catalog)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let target = self
            .router
            .open_target_for_catalog(&migration.target_key, &self.catalog)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let items = self
            .repository
            .items(self.database.write(), migration.id)
            .await?;
        for descriptor in self.catalog.tables() {
            let mut item = items
                .iter()
                .find(|item| item.table_name == descriptor.table)
                .cloned()
                .ok_or_else(|| AppError::Conflict("迁移表级检查点缺失".into()))?;
            item.state = tenant_data_migration_item::Model::STATE_VERIFYING.into();
            item.updated_at = Utc::now();
            item = self
                .repository
                .save_item(self.database.write(), item)
                .await?;
            let (source_count, source_digest) =
                self.table_digest(migration, &source, descriptor).await?;
            let (target_count, target_digest) =
                self.table_digest(migration, &target, descriptor).await?;
            if source_count != target_count || source_digest != target_digest {
                return Err(AppError::Conflict(format!(
                    "tenant-data verification failed for {}",
                    descriptor.table
                )));
            }
            verify_foreign_keys(migration, &source, descriptor).await?;
            verify_foreign_keys(migration, &target, descriptor).await?;
            item.source_row_count = Some(source_count);
            item.target_row_count = Some(target_count);
            item.source_digest = Some(source_digest);
            item.target_digest = Some(target_digest);
            item.state = tenant_data_migration_item::Model::STATE_VERIFIED.into();
            item.verified_at = Some(Utc::now());
            item.updated_at = Utc::now();
            self.repository
                .save_item(self.database.write(), item)
                .await?;
        }
        // 摘要与 FK 校验可能持续较久；进入不可取消边界前再做双目标实时
        // ping、MySQL 8.0.16+、ledger、完整 schema 与 mode/slot/fence 不变量复核。
        self.router
            .verify_target_now_for_catalog(&migration.source_target_key, &self.catalog)
            .await
            .map_err(crate::map_tenant_data_error)?;
        self.router
            .verify_target_now_for_catalog(&migration.target_key, &self.catalog)
            .await
            .map_err(crate::map_tenant_data_error)?;
        self.assert_migration_frozen_fence(migration, &migration.source_target_key)
            .await?;
        self.assert_migration_frozen_fence(migration, &migration.target_key)
            .await?;
        Ok(())
    }
    async fn copy_table_in_batches(
        &self,
        migration: &tenant_data_migration::Model,
        descriptor: &ryframe_tenant_db::migration::TenantDataTableDescriptor,
        source: &TenantDataTargetHandle,
        target: &TenantDataTargetHandle,
        mut item: tenant_data_migration_item::Model,
    ) -> AppResult<tenant_data_migration_item::Model> {
        let columns = descriptor.checksum_columns;
        let business_cursor = descriptor
            .primary_key_cursor_columns
            .iter()
            .copied()
            .filter(|column| *column != descriptor.tenant_column)
            .collect::<Vec<_>>();
        let mut cursor = item
            .cursor_json
            .as_ref()
            .and_then(JsonValue::as_array)
            .and_then(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect::<Option<Vec<_>>>()
            });
        if cursor.is_some() && item.source_digest.is_none() {
            return Err(AppError::Conflict(
                "迁移检查点有 cursor 但缺少滚动摘要，拒绝不安全续传".into(),
            ));
        }
        loop {
            self.assert_migration_frozen_fence(migration, &migration.source_target_key)
                .await?;
            let select_columns = columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    format!(
                        "IF(`{column}` IS NULL, NULL, TO_BASE64(CAST(`{column}` AS BINARY))) AS `c{index}`"
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let order = binary_cursor_columns(&business_cursor);
            let mut sql = format!(
                "SELECT {select_columns} FROM `{}` WHERE `{}` = ?",
                descriptor.table, descriptor.tenant_column
            );
            let mut values: Vec<sea_orm::Value> = vec![migration.tenant_id.clone().into()];
            if let Some(cursor_values) = &cursor {
                if cursor_values.len() != business_cursor.len() {
                    return Err(AppError::Conflict("迁移 cursor 与 catalog 不兼容".into()));
                }
                let cursor_columns = business_cursor
                    .iter()
                    .map(|column| format!("CAST(`{column}` AS BINARY)"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let parameters = business_cursor
                    .iter()
                    .map(|_| "FROM_BASE64(?)")
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!(" AND ({cursor_columns}) > ({parameters})"));
                values.extend(cursor_values.iter().cloned().map(Into::into));
            }
            sql.push_str(&format!(" ORDER BY {order} LIMIT {COPY_BATCH_SIZE}"));
            let rows = source
                .connection()
                .query_all_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    sql,
                    values,
                ))
                .await
                .map_err(database_error)?;
            if rows.is_empty() {
                break;
            }
            let decoded_rows = decode_rows(&rows, columns.len())?;
            write_target_batch(migration, descriptor, target, &decoded_rows).await?;
            cursor = Some(cursor_from_last_row(
                &decoded_rows,
                columns,
                &business_cursor,
            )?);
            let next_count = item
                .source_row_count
                .unwrap_or_default()
                .checked_add(i64::try_from(decoded_rows.len()).map_err(|_| {
                    AppError::Internal("tenant-data batch row count overflow".into())
                })?)
                .ok_or_else(|| AppError::Internal("tenant-data row count overflow".into()))?;
            let next_digest = rolling_digest(item.source_digest.as_deref(), &decoded_rows)?;
            // 目标批次提交后，再在控制库同一事务内续租并持久化 cursor/count/digest。
            // 若进程恰在两者之间退出，重放会命中幂等 upsert，并从旧 cursor 重新计算同一批。
            let progress_now = self
                .lease_repository
                .database_utc_now(self.database.write())
                .await?;
            let progress_transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            self.renew_operation_lease(&progress_transaction, migration, progress_now)
                .await?;
            let current = self
                .repository
                .lock_migration_in_txn(&progress_transaction, migration.id)
                .await?;
            ensure_not_cancel_requested(&current)?;
            if current.state != tenant_data_migration::Model::STATE_COPYING {
                return Err(AppError::TenantOperationConflict(
                    "复制批次提交时迁移状态已变化".into(),
                ));
            }
            item = self
                .repository
                .lock_item_in_txn(&progress_transaction, item.id)
                .await?;
            item.cursor_json = cursor.as_ref().map(|values| json!(values));
            item.source_row_count = Some(next_count);
            item.target_row_count = Some(next_count);
            item.source_digest = Some(next_digest.clone());
            item.target_digest = Some(next_digest);
            item.updated_at = progress_now;
            item = self
                .repository
                .save_item(&progress_transaction, item)
                .await?;
            progress_transaction
                .commit()
                .await
                .map_err(database_error)?;
            if decoded_rows.len() < COPY_BATCH_SIZE {
                break;
            }
        }
        Ok(item)
    }

    async fn table_digest(
        &self,
        migration: &tenant_data_migration::Model,
        target: &TenantDataTargetHandle,
        descriptor: &ryframe_tenant_db::migration::TenantDataTableDescriptor,
    ) -> AppResult<(i64, String)> {
        table_digest_in_batches(self, migration, target, descriptor).await
    }
}

const COPY_BATCH_SIZE: usize = 500;

async fn write_target_batch(
    migration: &tenant_data_migration::Model,
    descriptor: &ryframe_tenant_db::migration::TenantDataTableDescriptor,
    target: &TenantDataTargetHandle,
    rows: &[Vec<Option<String>>],
) -> AppResult<()> {
    let transaction = target.connection().begin().await.map_err(database_error)?;
    lock_target_fence(&transaction, migration, target).await?;
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
}

async fn lock_target_fence(
    transaction: &DatabaseTransaction,
    migration: &tenant_data_migration::Model,
    target: &TenantDataTargetHandle,
) -> AppResult<()> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT target_key, placement_generation, state, switch_token FROM biz_tenant_fence WHERE tenant_id = ? FOR UPDATE",
            [migration.tenant_id.clone().into()],
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
    if target_key != migration.target_key
        || generation != migration.target_generation
        || state != "frozen"
        || token != migration.switch_token
    {
        return Err(AppError::StalePlacementGeneration(
            "迁移目标 fence 已变化".into(),
        ));
    }
    if target.is_dedicated() {
        let slot = transaction
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                "SELECT tenant_id, placement_generation, switch_token FROM biz_tenant_target_slot WHERE slot_id = 1 FOR UPDATE",
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("dedicated target slot 不存在".into(), 5))?;
        let tenant_id: Option<String> = slot.try_get("", "tenant_id").map_err(database_error)?;
        let slot_generation: Option<i64> = slot
            .try_get("", "placement_generation")
            .map_err(database_error)?;
        let slot_token: Option<String> =
            slot.try_get("", "switch_token").map_err(database_error)?;
        if tenant_id.as_deref() != Some(&migration.tenant_id)
            || slot_generation != Some(migration.target_generation)
            || slot_token.as_deref() != Some(&migration.switch_token)
        {
            return Err(AppError::TenantOperationConflict(
                "dedicated target slot 已变化".into(),
            ));
        }
    }
    Ok(())
}

async fn table_digest_in_batches(
    service: &TenantDataMigrationService,
    migration: &tenant_data_migration::Model,
    target: &TenantDataTargetHandle,
    descriptor: &ryframe_tenant_db::migration::TenantDataTableDescriptor,
) -> AppResult<(i64, String)> {
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
    let business_cursor = descriptor
        .primary_key_cursor_columns
        .iter()
        .copied()
        .filter(|column| *column != descriptor.tenant_column)
        .collect::<Vec<_>>();
    let order = binary_cursor_columns(&business_cursor);
    let mut cursor: Option<Vec<String>> = None;
    let mut count = 0_i64;
    let mut digest: Option<String> = None;
    loop {
        service
            .assert_migration_frozen_fence(migration, target.target_key())
            .await?;
        let mut sql = format!(
            "SELECT {select_columns} FROM `{}` WHERE `{}` = ?",
            descriptor.table, descriptor.tenant_column
        );
        let mut values: Vec<sea_orm::Value> = vec![migration.tenant_id.clone().into()];
        if let Some(cursor_values) = &cursor {
            let cursor_columns = business_cursor
                .iter()
                .map(|column| format!("CAST(`{column}` AS BINARY)"))
                .collect::<Vec<_>>()
                .join(", ");
            let parameters = business_cursor
                .iter()
                .map(|_| "FROM_BASE64(?)")
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!(" AND ({cursor_columns}) > ({parameters})"));
            values.extend(cursor_values.iter().cloned().map(Into::into));
        }
        sql.push_str(&format!(" ORDER BY {order} LIMIT {COPY_BATCH_SIZE}"));
        let rows = target
            .connection()
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                sql,
                values,
            ))
            .await
            .map_err(database_error)?;
        if rows.is_empty() {
            break;
        }
        let decoded_rows = decode_rows(&rows, descriptor.checksum_columns.len())?;
        digest = Some(rolling_digest(digest.as_deref(), &decoded_rows)?);
        count = count
            .checked_add(i64::try_from(decoded_rows.len()).map_err(|_| {
                AppError::Internal("tenant-data verification batch overflow".into())
            })?)
            .ok_or_else(|| AppError::Internal("tenant-data verification count overflow".into()))?;
        cursor = Some(cursor_from_last_row(
            &decoded_rows,
            descriptor.checksum_columns,
            &business_cursor,
        )?);
        // 验证同样以批次为边界续租，不让长表摘要把统一租约拖过期。
        service
            .assert_worker_can_run(migration, tenant_data_migration::Model::STATE_VERIFYING)
            .await?;
        if decoded_rows.len() < COPY_BATCH_SIZE {
            break;
        }
    }
    Ok((count, digest.unwrap_or_else(empty_rolling_digest)))
}

fn binary_cursor_columns(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| format!("CAST(`{column}` AS BINARY)"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn verify_foreign_keys(
    migration: &tenant_data_migration::Model,
    target: &TenantDataTargetHandle,
    descriptor: &ryframe_tenant_db::migration::TenantDataTableDescriptor,
) -> AppResult<()> {
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
            .ok_or_else(|| AppError::Config("catalog FK 缺少 referenced tenant_id".into()))?;
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
                [migration.tenant_id.clone().into()],
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("tenant-data FK 校验无结果".into()))?;
        if row.try_get_by_index::<i64>(0).map_err(database_error)? != 0 {
            return Err(AppError::Conflict(format!(
                "tenant-data foreign-key verification failed for {}.{}",
                descriptor.table, foreign_key.name
            )));
        }
    }
    Ok(())
}

fn decode_rows(
    rows: &[sea_orm::QueryResult],
    column_count: usize,
) -> AppResult<Vec<Vec<Option<String>>>> {
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
    rows: &[Vec<Option<String>>],
    checksum_columns: &[&str],
    cursor_columns: &[&str],
) -> AppResult<Vec<String>> {
    let last = rows
        .last()
        .ok_or_else(|| AppError::Internal("empty tenant-data batch has no cursor".into()))?;
    cursor_columns
        .iter()
        .map(|column| {
            let index = checksum_columns
                .iter()
                .position(|candidate| candidate == column)
                .ok_or_else(|| AppError::Conflict("PK cursor 不在 checksum columns".into()))?;
            last[index]
                .clone()
                .ok_or_else(|| AppError::Conflict("PK cursor 不能为 NULL".into()))
        })
        .collect()
}

fn rolling_digest(previous: Option<&str>, rows: &[Vec<Option<String>>]) -> AppResult<String> {
    let mut state = [0_u8; 32];
    if let Some(previous) = previous {
        let decoded =
            hex::decode(previous).map_err(|_| AppError::Conflict("迁移滚动摘要格式无效".into()))?;
        if decoded.len() != state.len() {
            return Err(AppError::Conflict("迁移滚动摘要长度无效".into()));
        }
        state.copy_from_slice(&decoded);
    }
    for row in rows {
        let mut hasher = Sha256::new();
        hasher.update(state);
        for value in row {
            match value {
                Some(value) => {
                    hasher.update([1]);
                    hasher.update(
                        u64::try_from(value.len())
                            .map_err(|_| AppError::Internal("cell length overflow".into()))?
                            .to_be_bytes(),
                    );
                    hasher.update(value.as_bytes());
                }
                None => hasher.update([0]),
            }
        }
        state.copy_from_slice(&hasher.finalize());
    }
    Ok(hex::encode(state))
}

fn empty_rolling_digest() -> String {
    hex::encode([0_u8; 32])
}
