use std::{collections::BTreeSet, sync::Arc};

use crate::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, ControlDatabaseCluster,
    TenantConfigTransferRepository,
    entities::{
        background_job, tenant, tenant_config_bundle, tenant_config_transfer,
        tenant_config_transfer_item, tenant_operation_lease,
    },
};
use ryframe_kernel::{AppError, PageResult};
use sea_orm::{
    ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};

use super::super::transaction::DatabasePortTransaction;

use ryframe_application::{
    PersistenceFuture,
    ports::tenant_config::{
        TenantConfigBundleRecord, TenantConfigOperationLeaseRecord, TenantConfigRequesterRecord,
        TenantConfigTransferItemRecord, TenantConfigTransferPersistencePort,
        TenantConfigTransferRecord, TenantConfigTransferTransaction,
        TenantConfigurationFenceRecord,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantConfigTransferPersistencePort> {
    Arc::new(DatabaseTenantConfigTransferPersistence { database })
}

struct DatabaseTenantConfigTransferPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseTenantConfigTransferTransaction {
    transaction: DatabasePortTransaction,
}

impl TenantConfigTransferPersistencePort for DatabaseTenantConfigTransferPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn bundle_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ryframe_kernel::ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<TenantConfigBundleRecord>> {
        Box::pin(async move {
            let total = tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                .count(self.database.write())
                .await
                .map_err(database_error)?;
            let records = TenantConfigTransferRepository
                .list_bundles(
                    self.database.write(),
                    tenant_id,
                    page.page_size(),
                    page.offset(),
                )
                .await?
                .into_iter()
                .map(Into::into)
                .collect();
            Ok(PageResult::new(records, total, &page))
        })
    }

    fn transfer_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ryframe_kernel::ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<TenantConfigTransferRecord>> {
        Box::pin(async move {
            let total = tenant_config_transfer::Entity::find()
                .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
                .count(self.database.write())
                .await
                .map_err(database_error)?;
            let records = TenantConfigTransferRepository
                .list_transfers(
                    self.database.write(),
                    tenant_id,
                    page.page_size(),
                    page.offset(),
                )
                .await?
                .into_iter()
                .map(Into::into)
                .collect();
            Ok(PageResult::new(records, total, &page))
        })
    }

    fn item_page<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        page: ryframe_kernel::ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<TenantConfigTransferItemRecord>> {
        Box::pin(async move {
            let query = tenant_config_transfer_item::Entity::find()
                .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
                .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id));
            let total = query
                .clone()
                .count(self.database.write())
                .await
                .map_err(database_error)?;
            let records = query
                .order_by_asc(tenant_config_transfer_item::Column::Id)
                .limit(page.page_size())
                .offset(page.offset())
                .all(self.database.write())
                .await
                .map_err(database_error)?
                .into_iter()
                .map(Into::into)
                .collect();
            Ok(PageResult::new(records, total, &page))
        })
    }

    fn find_bundle<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<TenantConfigBundleRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .find_bundle_by_id(self.database.write(), tenant_id, id)
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn find_bundles<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<TenantConfigBundleRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .find_bundles_by_ids(self.database.write(), tenant_id, ids)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
        })
    }

    fn find_transfer<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<TenantConfigTransferRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .find_transfer_by_id(self.database.write(), tenant_id, id)
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn find_transfer_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<TenantConfigTransferRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .find_transfer_by_background_job(self.database.write(), background_job_id)
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn find_transfer_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        requested_by: i64,
        idempotency_key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantConfigTransferRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .find_transfer_by_idempotency_key(
                    self.database.write(),
                    tenant_id,
                    requested_by,
                    idempotency_key_hash,
                )
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> PersistenceFuture<'a, Vec<TenantConfigTransferItemRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .list_items(self.database.write(), tenant_id, transfer_id)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
        })
    }

    fn cache_namespace_version<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, i64> {
        Box::pin(async move {
            CacheNamespaceVersionRepository
                .find_version(self.database.write(), tenant_id, CONFIG_CACHE_NAMESPACE)
                .await
        })
    }

    fn load_resources<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ryframe_application::system::TenantConfigPackageResources> {
        Box::pin(async move {
            super::transfer_sql::load_resources_on(self.database.write(), tenant_id).await
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn TenantConfigTransferTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseTenantConfigTransferTransaction {
                transaction: transaction.into(),
            }) as Box<dyn TenantConfigTransferTransaction>)
        })
    }
}

impl TenantConfigTransferTransaction for DatabaseTenantConfigTransferTransaction {
    fn background_jobs(&self) -> &dyn ryframe_application::ports::jobs::BackgroundJobTransaction {
        &self.transaction
    }

    fn product(&self) -> &dyn ryframe_application::ports::product::ProductTransactionPort {
        &self.transaction
    }

    fn authorization_mirror(
        &self,
    ) -> &dyn ryframe_application::ports::authorization::AuthorizationMirrorTransaction {
        &self.transaction
    }

    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn lock_tenant_configuration<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: Option<&'a str>,
    ) -> PersistenceFuture<'a, TenantConfigurationFenceRecord> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, owner_token)
                .await
                .map(Into::into)
        })
    }

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i64> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&self.transaction, tenant_id)
                .await
        })
    }

    fn acquire_lease(&self, lease: TenantConfigOperationLeaseRecord) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .acquire_lease_in_txn(&self.transaction, lease.into())
                .await
                .map(|_| ())
        })
    }

    fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .renew_lease_in_txn(&self.transaction, tenant_id, owner_token, expires_at)
                .await
        })
    }

    fn release_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .release_lease_in_txn(&self.transaction, tenant_id, owner_token)
                .await
        })
    }

    fn insert_bundle(
        &self,
        bundle: TenantConfigBundleRecord,
    ) -> PersistenceFuture<'_, TenantConfigBundleRecord> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .insert_bundle(&self.transaction, bundle.into())
                .await
                .map(Into::into)
        })
    }

    fn lock_bundle<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<TenantConfigBundleRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_bundle_in_txn(&self.transaction, tenant_id, id)
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn lock_bundle_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<TenantConfigBundleRecord>> {
        Box::pin(async move {
            tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::BackgroundJobId.eq(background_job_id))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)
                .map(|record| record.map(Into::into))
        })
    }

    fn find_bundle_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        created_by: i64,
        idempotency_key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantConfigBundleRecord>> {
        Box::pin(async move {
            tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                .filter(tenant_config_bundle::Column::CreatedBy.eq(created_by))
                .filter(tenant_config_bundle::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
                .one(&self.transaction)
                .await
                .map_err(database_error)
                .map(|record| record.map(Into::into))
        })
    }

    fn update_bundle(
        &self,
        bundle: TenantConfigBundleRecord,
    ) -> PersistenceFuture<'_, TenantConfigBundleRecord> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .update_bundle(&self.transaction, bundle.into())
                .await
                .map(Into::into)
        })
    }

    fn insert_transfer(
        &self,
        transfer: TenantConfigTransferRecord,
    ) -> PersistenceFuture<'_, TenantConfigTransferRecord> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .insert_transfer(&self.transaction, transfer.into())
                .await
                .map(Into::into)
        })
    }

    fn find_transfer_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        requested_by: i64,
        idempotency_key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantConfigTransferRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .find_transfer_by_idempotency_key(
                    &self.transaction,
                    tenant_id,
                    requested_by,
                    idempotency_key_hash,
                )
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn lock_transfer<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<TenantConfigTransferRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_transfer_in_txn(&self.transaction, tenant_id, id)
                .await
                .map(|record| record.map(Into::into))
        })
    }

    fn update_transfer(
        &self,
        transfer: TenantConfigTransferRecord,
    ) -> PersistenceFuture<'_, TenantConfigTransferRecord> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .update_transfer(&self.transaction, transfer.into())
                .await
                .map(Into::into)
        })
    }

    fn replace_items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        items: Vec<TenantConfigTransferItemRecord>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .replace_items_in_txn(
                    &self.transaction,
                    tenant_id,
                    transfer_id,
                    items.into_iter().map(Into::into).collect(),
                )
                .await
        })
    }

    fn list_items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> PersistenceFuture<'a, Vec<TenantConfigTransferItemRecord>> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .list_items(&self.transaction, tenant_id, transfer_id)
                .await
                .map(|records| records.into_iter().map(Into::into).collect())
        })
    }

    fn tenant_name<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, String> {
        Box::pin(async move {
            tenant::Entity::find()
                .filter(tenant::Column::TenantId.eq(tenant_id))
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(|tenant| tenant.name)
                .ok_or_else(|| AppError::NotFound("租户不存在".into()))
        })
    }

    fn ensure_config_package_file_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::ensure_config_package_file_ready_in_txn(
                &self.transaction,
                tenant_id,
                file_id,
                now,
            )
            .await
        })
    }

    fn load_resources<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ryframe_application::system::TenantConfigPackageResources> {
        Box::pin(async move {
            super::transfer_sql::load_resources_on(&self.transaction, tenant_id).await
        })
    }

    fn apply_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ryframe_application::system::TenantConfigPackageResources,
        plan_items: &'a [TenantConfigTransferItemRecord],
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::apply_resources_in_transaction(
                &self.transaction,
                tenant_id,
                resources,
                plan_items,
                now,
            )
            .await
        })
    }

    fn ensure_rollback_references_safe<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::ensure_rollback_references_safe(
                &self.transaction,
                tenant_id,
                transfer_id,
            )
            .await
        })
    }

    fn restore_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        snapshot: &'a ryframe_application::system::TenantConfigPackageResources,
        transfer_id: i64,
        target_catalog: &'a ryframe_application::system::TenantConfigTargetCatalog,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::restore_snapshot_in_transaction(
                &self.transaction,
                tenant_id,
                snapshot,
                transfer_id,
                target_catalog,
                now,
            )
            .await
        })
    }

    fn ensure_requester_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        requester: TenantConfigRequesterRecord,
        fence: TenantConfigurationFenceRecord,
        database_now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::ensure_requester_snapshot_in_txn(
                &self.transaction,
                tenant_id,
                requester,
                fence,
                database_now,
            )
            .await
        })
    }

    fn ensure_role_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        plan_items: &'a [TenantConfigTransferItemRecord],
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::ensure_role_quota_for_plan_in_txn(
                &self.transaction,
                tenant_id,
                plan_items,
            )
            .await
        })
    }

    fn mark_plan_outcome<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        outcome: &'a str,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            super::transfer_sql::mark_plan_outcome(
                &self.transaction,
                tenant_id,
                transfer_id,
                outcome,
            )
            .await
        })
    }

    fn dead_background_job_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        candidates: &'a [i64],
    ) -> PersistenceFuture<'a, BTreeSet<i64>> {
        Box::pin(async move {
            if candidates.is_empty() {
                return Ok(BTreeSet::new());
            }
            background_job::Entity::find()
                .filter(background_job::Column::Id.is_in(candidates.iter().copied()))
                .filter(background_job::Column::TenantId.eq(tenant_id))
                .filter(background_job::Column::Status.eq(background_job::Model::STATUS_DEAD))
                .all(&self.transaction)
                .await
                .map_err(database_error)
                .map(|jobs| jobs.into_iter().map(|job| job.id).collect())
        })
    }

    fn commit_audited(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit_audited().await })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit().await.map_err(database_error) })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

impl From<crate::TenantConfigurationFence> for TenantConfigurationFenceRecord {
    fn from(value: crate::TenantConfigurationFence) -> Self {
        Self {
            configuration_version: value.configuration_version,
            authorization_epoch: value.authorization_epoch,
        }
    }
}

impl From<TenantConfigOperationLeaseRecord> for tenant_operation_lease::Model {
    fn from(value: TenantConfigOperationLeaseRecord) -> Self {
        Self {
            tenant_id: value.tenant_id,
            owner_token: value.owner_token,
            operation: value.operation,
            resource_type: value.resource_type,
            resource_id: value.resource_id,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<tenant_config_bundle::Model> for TenantConfigBundleRecord {
    fn from(value: tenant_config_bundle::Model) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name_snapshot: value.source_tenant_name_snapshot,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            file_id: value.file_id,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            background_job_id: value.background_job_id,
            idempotency_key_hash: value.idempotency_key_hash,
            created_by: value.created_by,
            error_summary: value.error_summary,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<TenantConfigBundleRecord> for tenant_config_bundle::Model {
    fn from(value: TenantConfigBundleRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name_snapshot: value.source_tenant_name_snapshot,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            file_id: value.file_id,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            background_job_id: value.background_job_id,
            idempotency_key_hash: value.idempotency_key_hash,
            created_by: value.created_by,
            error_summary: value.error_summary,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<tenant_config_transfer::Model> for TenantConfigTransferRecord {
    fn from(value: tenant_config_transfer::Model) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            bundle_id: value.bundle_id,
            idempotency_key_hash: value.idempotency_key_hash,
            request_kind: value.request_kind,
            request_fingerprint: value.request_fingerprint,
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch,
            plan_hash: value.plan_hash,
            preview_calculated_at: value.preview_calculated_at,
            preview_background_job_id: value.preview_background_job_id,
            apply_background_job_id: value.apply_background_job_id,
            rollback_background_job_id: value.rollback_background_job_id,
            snapshot_file_id: value.snapshot_file_id,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value.applied_authorization_epoch,
            change_counts: value.change_counts,
            error_summary: value.error_summary,
            requested_by: value.requested_by,
            rollback_expires_at: value.rollback_expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<TenantConfigTransferRecord> for tenant_config_transfer::Model {
    fn from(value: TenantConfigTransferRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            bundle_id: value.bundle_id,
            idempotency_key_hash: value.idempotency_key_hash,
            request_kind: value.request_kind,
            request_fingerprint: value.request_fingerprint,
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch,
            plan_hash: value.plan_hash,
            preview_calculated_at: value.preview_calculated_at,
            preview_background_job_id: value.preview_background_job_id,
            apply_background_job_id: value.apply_background_job_id,
            rollback_background_job_id: value.rollback_background_job_id,
            snapshot_file_id: value.snapshot_file_id,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value.applied_authorization_epoch,
            change_counts: value.change_counts,
            error_summary: value.error_summary,
            requested_by: value.requested_by,
            rollback_expires_at: value.rollback_expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<tenant_config_transfer_item::Model> for TenantConfigTransferItemRecord {
    fn from(value: tenant_config_transfer_item::Model) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            transfer_id: value.transfer_id,
            resource_type: value.resource_type,
            stable_key: value.stable_key,
            display_name: value.display_name,
            action: value.action,
            outcome: value.outcome,
            detail_code: value.detail_code,
            detail: value.detail,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<TenantConfigTransferItemRecord> for tenant_config_transfer_item::Model {
    fn from(value: TenantConfigTransferItemRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            transfer_id: value.transfer_id,
            resource_type: value.resource_type,
            stable_key: value.stable_key,
            display_name: value.display_name,
            action: value.action,
            outcome: value.outcome,
            detail_code: value.detail_code,
            detail: value.detail,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;

    #[test]
    fn bundle_mapping_preserves_every_field() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 2, 3, 4).unwrap();
        let expected = tenant_config_bundle::Model {
            id: 7,
            tenant_id: "tenant-a".into(),
            origin: "uploaded".into(),
            source_tenant_key: "source-a".into(),
            source_tenant_name_snapshot: "来源租户".into(),
            package_schema_version: "1".into(),
            source_app_version: "0.10.0".into(),
            file_id: Some(8),
            sha256: Some("a".repeat(64)),
            resource_counts: json!({"role": 2}),
            item_count: 2,
            status: "succeeded".into(),
            background_job_id: Some(9),
            idempotency_key_hash: Some("b".repeat(64)),
            created_by: 10,
            error_summary: Some("摘要".into()),
            expires_at: Some(now),
            created_at: now,
            updated_at: now,
        };

        let restored: tenant_config_bundle::Model =
            TenantConfigBundleRecord::from(expected.clone()).into();

        assert_eq!(restored, expected);
    }

    #[test]
    fn transfer_mapping_preserves_every_field() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 3, 4, 5).unwrap();
        let expected = tenant_config_transfer::Model {
            id: 11,
            tenant_id: "tenant-a".into(),
            bundle_id: 12,
            idempotency_key_hash: "c".repeat(64),
            request_kind: "upload".into(),
            request_fingerprint: "d".repeat(64),
            status: "previewed".into(),
            target_configuration_version: 13,
            target_authorization_epoch: 14,
            plan_hash: Some("e".repeat(64)),
            preview_calculated_at: Some(now),
            preview_background_job_id: Some(15),
            apply_background_job_id: Some(16),
            rollback_background_job_id: Some(17),
            snapshot_file_id: Some(18),
            applied_configuration_version: Some(19),
            applied_authorization_epoch: Some(20),
            change_counts: json!({"create": 3}),
            error_summary: Some("错误".into()),
            requested_by: 21,
            rollback_expires_at: Some(now),
            created_at: now,
            updated_at: now,
        };

        let restored: tenant_config_transfer::Model =
            TenantConfigTransferRecord::from(expected.clone()).into();

        assert_eq!(restored, expected);
    }

    #[test]
    fn item_and_fence_mapping_preserve_boundaries() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 4, 5, 6).unwrap();
        let expected = tenant_config_transfer_item::Model {
            id: 22,
            tenant_id: "tenant-a".into(),
            transfer_id: 23,
            resource_type: "role".into(),
            stable_key: "role:admin".into(),
            display_name: "管理员".into(),
            action: "update".into(),
            outcome: "applied".into(),
            detail_code: Some("changed".into()),
            detail: Some("已更新".into()),
            created_at: now,
            updated_at: now,
        };
        let restored: tenant_config_transfer_item::Model =
            TenantConfigTransferItemRecord::from(expected.clone()).into();
        let fence = TenantConfigurationFenceRecord::from(crate::TenantConfigurationFence {
            configuration_version: 24,
            authorization_epoch: 25,
        });

        assert_eq!(restored, expected);
        assert_eq!(fence.configuration_version, 24);
        assert_eq!(fence.authorization_epoch, 25);
    }
}
