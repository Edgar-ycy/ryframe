use std::collections::HashMap;

use chrono::Utc;
use ryframe_db::{tenant_data_migration, tenant_data_migration_item};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::TransactionTrait;
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};

use crate::{TenantDataCatalogTable, TenantDataFence, TenantDataRow};

use super::workflow::ensure_not_cancel_requested;
use super::{TenantDataMigrationService, database_error};

impl TenantDataMigrationService {
    pub(super) async fn copy_catalog(
        &self,
        migration: &tenant_data_migration::Model,
    ) -> AppResult<()> {
        self.targets
            .validate_catalog(&migration.source_target_key)
            .await?;
        self.targets.validate_catalog(&migration.target_key).await?;
        let existing = self
            .repository
            .items(self.database.write(), migration.id)
            .await?;
        let mut existing_by_table = existing
            .into_iter()
            .map(|item| (item.table_name.clone(), item))
            .collect::<HashMap<_, _>>();
        for descriptor in self.tenant_migration.catalog_tables() {
            let mut item = if let Some(item) = existing_by_table.remove(descriptor.name) {
                item
            } else {
                let now = Utc::now();
                self.repository
                    .insert_item(
                        self.database.write(),
                        tenant_data_migration_item::Model {
                            id: crate::next_id()?,
                            migration_id: migration.id,
                            table_name: descriptor.name.into(),
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
                .copy_table_in_batches(migration, descriptor, item)
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
        self.targets
            .validate_catalog(&migration.source_target_key)
            .await?;
        self.targets.validate_catalog(&migration.target_key).await?;
        let items = self
            .repository
            .items(self.database.write(), migration.id)
            .await?;
        let mut items_by_table = items
            .into_iter()
            .map(|item| (item.table_name.clone(), item))
            .collect::<HashMap<_, _>>();
        for descriptor in self.tenant_migration.catalog_tables() {
            let mut item = items_by_table
                .remove(descriptor.name)
                .ok_or_else(|| AppError::Conflict("迁移表级检查点缺失".into()))?;
            item.state = tenant_data_migration_item::Model::STATE_VERIFYING.into();
            item.updated_at = Utc::now();
            item = self
                .repository
                .save_item(self.database.write(), item)
                .await?;
            let (source_count, source_digest) = self
                .table_digest(migration, &migration.source_target_key, descriptor)
                .await?;
            let (target_count, target_digest) = self
                .table_digest(migration, &migration.target_key, descriptor)
                .await?;
            if source_count != target_count || source_digest != target_digest {
                return Err(AppError::Conflict(format!(
                    "租户数据表校验失败: {}",
                    descriptor.name
                )));
            }
            self.tenant_migration
                .verify_foreign_keys(
                    &migration.source_target_key,
                    &migration.tenant_id,
                    descriptor.name,
                )
                .await?;
            self.tenant_migration
                .verify_foreign_keys(&migration.target_key, &migration.tenant_id, descriptor.name)
                .await?;
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
        // 摘要与外键校验可能持续较久；进入不可取消边界前重新验证目标与 fence。
        self.targets
            .verify_now(&migration.source_target_key)
            .await?;
        self.targets.verify_now(&migration.target_key).await?;
        self.assert_migration_frozen_fence(migration, &migration.source_target_key)
            .await?;
        self.assert_migration_frozen_fence(migration, &migration.target_key)
            .await?;
        Ok(())
    }

    async fn copy_table_in_batches(
        &self,
        migration: &tenant_data_migration::Model,
        descriptor: TenantDataCatalogTable,
        mut item: tenant_data_migration_item::Model,
    ) -> AppResult<tenant_data_migration_item::Model> {
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
                "迁移检查点有 cursor 但缺少滚动摘要".into(),
            ));
        }
        loop {
            self.assert_migration_frozen_fence(migration, &migration.source_target_key)
                .await?;
            let batch = self
                .tenant_migration
                .read_rows_batch(
                    &migration.source_target_key,
                    &migration.tenant_id,
                    descriptor.name,
                    cursor.as_deref(),
                    COPY_BATCH_SIZE,
                )
                .await?;
            if batch.rows.is_empty() {
                break;
            }
            self.tenant_migration
                .write_rows_batch(target_fence(migration)?, descriptor.name, &batch.rows)
                .await?;
            cursor = batch.next_cursor;
            let next_count = item
                .source_row_count
                .unwrap_or_default()
                .checked_add(
                    i64::try_from(batch.rows.len())
                        .map_err(|_| AppError::Internal("租户数据批次行数溢出".into()))?,
                )
                .ok_or_else(|| AppError::Internal("租户数据行数溢出".into()))?;
            let next_digest = rolling_digest(item.source_digest.as_deref(), &batch.rows)?;
            // 目标批次提交后，再在控制库同一事务内续租并持久化检查点。
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
            if batch.rows.len() < COPY_BATCH_SIZE as usize {
                break;
            }
        }
        Ok(item)
    }

    async fn table_digest(
        &self,
        migration: &tenant_data_migration::Model,
        target_key: &str,
        descriptor: TenantDataCatalogTable,
    ) -> AppResult<(i64, String)> {
        let mut cursor: Option<Vec<String>> = None;
        let mut count = 0_i64;
        let mut digest: Option<String> = None;
        loop {
            self.assert_migration_frozen_fence(migration, target_key)
                .await?;
            let batch = self
                .tenant_migration
                .read_rows_batch(
                    target_key,
                    &migration.tenant_id,
                    descriptor.name,
                    cursor.as_deref(),
                    COPY_BATCH_SIZE,
                )
                .await?;
            if batch.rows.is_empty() {
                break;
            }
            digest = Some(rolling_digest(digest.as_deref(), &batch.rows)?);
            count = count
                .checked_add(
                    i64::try_from(batch.rows.len())
                        .map_err(|_| AppError::Internal("租户数据校验批次行数溢出".into()))?,
                )
                .ok_or_else(|| AppError::Internal("租户数据校验行数溢出".into()))?;
            cursor = batch.next_cursor;
            self.assert_worker_can_run(migration, tenant_data_migration::Model::STATE_VERIFYING)
                .await?;
            if batch.rows.len() < COPY_BATCH_SIZE as usize {
                break;
            }
        }
        Ok((count, digest.unwrap_or_else(empty_rolling_digest)))
    }
}

const COPY_BATCH_SIZE: u32 = 500;

fn target_fence(migration: &tenant_data_migration::Model) -> AppResult<TenantDataFence<'_>> {
    Ok(TenantDataFence {
        tenant_id: &migration.tenant_id,
        target_key: &migration.target_key,
        generation: super::checked_generation(migration.target_generation, "目标")?,
        switch_token: &migration.switch_token,
    })
}

fn rolling_digest(previous: Option<&str>, rows: &[TenantDataRow]) -> AppResult<String> {
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
                            .map_err(|_| AppError::Internal("单元格长度溢出".into()))?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_digest_is_stable_and_order_sensitive() {
        let first = vec![vec![Some("a".into()), None], vec![Some("b".into())]];
        let second = vec![vec![Some("b".into())], vec![Some("a".into()), None]];
        assert_eq!(
            rolling_digest(None, &first).expect("应计算摘要"),
            rolling_digest(None, &first).expect("相同行应得到相同摘要")
        );
        assert_ne!(
            rolling_digest(None, &first).expect("应计算摘要"),
            rolling_digest(None, &second).expect("应计算摘要")
        );
    }
}
