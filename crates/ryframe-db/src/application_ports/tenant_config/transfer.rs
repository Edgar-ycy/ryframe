use crate::DbResultExt;
use std::{collections::BTreeSet, sync::Arc};

use crate::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, ControlDatabaseCluster,
    TenantConfigTransferRepository,
    entities::{
        background_job,
        tenant::{
            self, config_bundle as tenant_config_bundle, config_transfer as tenant_config_transfer,
            config_transfer_item as tenant_config_transfer_item,
        },
    },
};
use ryframe_kernel::{AppError, PageResult};
use sea_orm::{
    ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};

use super::super::transaction::DatabasePortTransaction;

use ryframe_application::ports::tenant_config::{
    TenantConfigBundleRecord, TenantConfigOperationLeaseRecord, TenantConfigRequesterRecord,
    TenantConfigTransferItemRecord, TenantConfigTransferPersistencePort,
    TenantConfigTransferRecord, TenantConfigTransferTransaction, TenantConfigurationFenceRecord,
};

mod read;
mod records;

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantConfigTransferPersistencePort> {
    Arc::new(DatabaseTenantConfigTransferPersistence { database })
}

struct DatabaseTenantConfigTransferPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseTenantConfigTransferTransaction {
    transaction: DatabasePortTransaction,
}

#[async_trait::async_trait]
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

    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn lock_tenant_configuration<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: Option<&'a str>,
    ) -> ryframe_kernel::AppResult<TenantConfigurationFenceRecord> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, owner_token)
            .await
            .map(Into::into)
    }

    async fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i64> {
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&self.transaction, tenant_id)
            .await
    }

    async fn acquire_lease(
        &self,
        lease: TenantConfigOperationLeaseRecord,
    ) -> ryframe_kernel::AppResult<()> {
        TenantConfigTransferRepository
            .acquire_lease_in_txn(&self.transaction, lease.into())
            .await
            .map(|_| ())
    }

    async fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        TenantConfigTransferRepository
            .renew_lease_in_txn(&self.transaction, tenant_id, owner_token, expires_at)
            .await
    }

    async fn release_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> ryframe_kernel::AppResult<bool> {
        TenantConfigTransferRepository
            .release_lease_in_txn(&self.transaction, tenant_id, owner_token)
            .await
    }

    async fn insert_bundle(
        &self,
        bundle: TenantConfigBundleRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigBundleRecord> {
        TenantConfigTransferRepository
            .insert_bundle(&self.transaction, bundle.into())
            .await
            .map(Into::into)
    }

    async fn lock_bundle<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>> {
        TenantConfigTransferRepository
            .lock_bundle_in_txn(&self.transaction, tenant_id, id)
            .await
            .map(|record| record.map(Into::into))
    }

    async fn lock_bundle_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>> {
        tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::BackgroundJobId.eq(background_job_id))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()
            .map(|record| record.map(Into::into))
    }

    async fn find_bundle_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        created_by: i64,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>> {
        tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_bundle::Column::CreatedBy.eq(created_by))
            .filter(tenant_config_bundle::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
            .one(&self.transaction)
            .await
            .db()
            .map(|record| record.map(Into::into))
    }

    async fn update_bundle(
        &self,
        bundle: TenantConfigBundleRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigBundleRecord> {
        TenantConfigTransferRepository
            .update_bundle(&self.transaction, bundle.into())
            .await
            .map(Into::into)
    }

    async fn insert_transfer(
        &self,
        transfer: TenantConfigTransferRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigTransferRecord> {
        TenantConfigTransferRepository
            .insert_transfer(&self.transaction, transfer.into())
            .await
            .map(Into::into)
    }

    async fn find_transfer_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        requested_by: i64,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>> {
        TenantConfigTransferRepository
            .find_transfer_by_idempotency_key(
                &self.transaction,
                tenant_id,
                requested_by,
                idempotency_key_hash,
            )
            .await
            .map(|record| record.map(Into::into))
    }

    async fn lock_transfer<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>> {
        TenantConfigTransferRepository
            .lock_transfer_in_txn(&self.transaction, tenant_id, id)
            .await
            .map(|record| record.map(Into::into))
    }

    async fn update_transfer(
        &self,
        transfer: TenantConfigTransferRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigTransferRecord> {
        TenantConfigTransferRepository
            .update_transfer(&self.transaction, transfer.into())
            .await
            .map(Into::into)
    }

    async fn replace_items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        items: Vec<TenantConfigTransferItemRecord>,
    ) -> ryframe_kernel::AppResult<()> {
        TenantConfigTransferRepository
            .replace_items_in_txn(
                &self.transaction,
                tenant_id,
                transfer_id,
                items.into_iter().map(Into::into).collect(),
            )
            .await
    }

    async fn list_items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<TenantConfigTransferItemRecord>> {
        TenantConfigTransferRepository
            .list_items(&self.transaction, tenant_id, transfer_id)
            .await
            .map(|records| records.into_iter().map(Into::into).collect())
    }

    async fn tenant_name<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<String> {
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .one(&self.transaction)
            .await
            .db()?
            .map(|tenant| tenant.name)
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    async fn ensure_config_package_file_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::ensure_config_package_file_ready_in_txn(
            &self.transaction,
            tenant_id,
            file_id,
            now,
        )
        .await
    }

    async fn load_resources<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<
        ryframe_application::system::platform::TenantConfigPackageResources,
    > {
        super::transfer_sql::load_resources_on(&self.transaction, tenant_id).await
    }

    async fn apply_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ryframe_application::system::platform::TenantConfigPackageResources,
        plan_items: &'a [TenantConfigTransferItemRecord],
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::apply_resources_in_transaction(
            &self.transaction,
            tenant_id,
            resources,
            plan_items,
            now,
        )
        .await
    }

    async fn ensure_rollback_references_safe<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::ensure_rollback_references_safe(
            &self.transaction,
            tenant_id,
            transfer_id,
        )
        .await
    }

    async fn restore_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        snapshot: &'a ryframe_application::system::platform::TenantConfigPackageResources,
        transfer_id: i64,
        target_catalog: &'a ryframe_application::system::platform::TenantConfigTargetCatalog,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::restore_snapshot_in_transaction(
            &self.transaction,
            tenant_id,
            snapshot,
            transfer_id,
            target_catalog,
            now,
        )
        .await
    }

    async fn ensure_requester_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        requester: TenantConfigRequesterRecord,
        fence: TenantConfigurationFenceRecord,
        database_now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::ensure_requester_snapshot_in_txn(
            &self.transaction,
            tenant_id,
            requester,
            fence,
            database_now,
        )
        .await
    }

    async fn ensure_role_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        plan_items: &'a [TenantConfigTransferItemRecord],
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::ensure_role_quota_for_plan_in_txn(
            &self.transaction,
            tenant_id,
            plan_items,
        )
        .await
    }

    async fn mark_plan_outcome<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        outcome: &'a str,
    ) -> ryframe_kernel::AppResult<()> {
        super::transfer_sql::mark_plan_outcome(&self.transaction, tenant_id, transfer_id, outcome)
            .await
    }

    async fn dead_background_job_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        candidates: &'a [i64],
    ) -> ryframe_kernel::AppResult<BTreeSet<i64>> {
        if candidates.is_empty() {
            return Ok(BTreeSet::new());
        }
        background_job::Entity::find()
            .filter(background_job::Column::Id.is_in(candidates.iter().copied()))
            .filter(background_job::Column::TenantId.eq(tenant_id))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_DEAD))
            .all(&self.transaction)
            .await
            .db()
            .map(|jobs| jobs.into_iter().map(|job| job.id).collect())
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseTenantConfigTransferTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            ryframe_application::TransactionAuditMode::CurrentRequest => {
                self.transaction.commit_audited().await
            }
            ryframe_application::TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.db()
    }
}
