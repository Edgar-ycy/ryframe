use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, CreateUserImportJob, DeptRepository, FileRepository,
    TenantConfigTransferRepository, TenantRepository, UserImportFilter, UserImportRepository,
    UserRepository,
    entities::{user, user_import_job},
};

mod model;

use model::{import_row_model, job_model, job_record, row_record, source_record, user_model};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use sea_orm::{EntityTrait, TransactionTrait};

use super::super::transaction::DatabasePortTransaction;

use ryframe_application::{
    EnqueueJob, EnqueueJobResult,
    ports::jobs::BackgroundJobTransaction,
    ports::users::{
        NewImportedUser, NewUserImportJob, NewUserImportRow, UserImportAuthorizationSnapshot,
        UserImportDepartmentRecord, UserImportJobRecord, UserImportPersistencePort,
        UserImportReadFilter, UserImportRowRecord, UserImportSourceRecord, UserImportTransaction,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn UserImportPersistencePort> {
    Arc::new(DatabaseUserImportPersistence { database })
}

struct DatabaseUserImportPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseUserImportTransaction {
    transaction: DatabasePortTransaction,
}

#[async_trait::async_trait]
impl UserImportPersistencePort for DatabaseUserImportPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn UserImportTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseUserImportTransaction {
            transaction: transaction.into(),
        }) as Box<dyn UserImportTransaction>)
    }

    async fn list_departments<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Vec<UserImportDepartmentRecord>> {
        DeptRepository
            .find_filtered(self.database.write(), tenant_id, None, None)
            .await
            .map(|departments| {
                departments
                    .into_iter()
                    .map(|department| UserImportDepartmentRecord {
                        id: department.id,
                        name: department.name,
                        parent_id: department.parent_id,
                        ancestors: department.ancestors,
                        status: department.status,
                    })
                    .collect()
            })
    }

    async fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: UserImportReadFilter<'a>,
    ) -> ryframe_kernel::AppResult<PageResult<UserImportJobRecord>> {
        let result = UserImportRepository
            .list_for_tenant(
                self.database.write(),
                tenant_id,
                &page,
                UserImportFilter {
                    status: filter.status,
                },
            )
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(job_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>> {
        UserImportRepository
            .find_by_id_for_tenant(self.database.write(), tenant_id, import_id)
            .await
            .map(|job| job.map(job_record))
    }

    async fn find_global(
        &self,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>> {
        user_import_job::Entity::find_by_id(import_id)
            .one(self.database.write())
            .await
            .map(|job| job.map(job_record))
            .db()
    }

    async fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>> {
        UserImportRepository
            .find_by_background_job(self.database.write(), background_job_id)
            .await
            .map(|job| job.map(job_record))
    }

    async fn rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<UserImportRowRecord>> {
        let result = UserImportRepository
            .list_row_results(self.database.write(), tenant_id, import_id, &page)
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(row_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn all_rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<UserImportRowRecord>> {
        UserImportRepository
            .all_row_results(self.database.write(), tenant_id, import_id)
            .await
            .map(|rows| rows.into_iter().map(row_record).collect())
    }

    async fn requester_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<(i64, String)>> {
        UserRepository
            .find_usernames_by_ids(self.database.write(), tenant_id, user_ids)
            .await
    }

    async fn request_cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<bool> {
        let now = crate::repositories::database_utc_now(self.database.write()).await?;
        UserImportRepository
            .request_cancel(self.database.write(), tenant_id, import_id, now)
            .await
    }
}

#[async_trait::async_trait]
impl BackgroundJobTransaction for DatabaseUserImportTransaction {
    async fn enqueue(&self, command: EnqueueJob) -> ryframe_kernel::AppResult<EnqueueJobResult> {
        <DatabasePortTransaction as BackgroundJobTransaction>::enqueue(&self.transaction, command)
            .await
    }

    async fn reactivate_linked<'a>(
        &'a self,
        job_id: i64,
        expected_job_type: &'a str,
        payload_key: &'a str,
        expected_resource_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        <DatabasePortTransaction as BackgroundJobTransaction>::reactivate_linked(
            &self.transaction,
            job_id,
            expected_job_type,
            payload_key,
            expected_resource_id,
            now,
        )
        .await
    }
}

#[async_trait::async_trait]
impl UserImportTransaction for DatabaseUserImportTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }

    async fn find_by_idempotency<'a>(
        &'a self,
        tenant_id: &'a str,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>> {
        UserImportRepository
            .find_by_idempotency_in_txn(&self.transaction, tenant_id, idempotency_key_hash)
            .await
            .map(|job| job.map(job_record))
    }

    async fn requester_username<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<String>> {
        UserRepository
            .find_usernames_by_ids(&self.transaction, tenant_id, &[user_id])
            .await
            .map(|users| users.into_iter().next().map(|(_, username)| username))
    }

    async fn active_count<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<u64> {
        UserImportRepository
            .count_active_in_txn(&self.transaction, tenant_id)
            .await
    }

    async fn lock_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<UserImportSourceRecord>> {
        FileRepository
            .find_by_id_any_status_for_update(&self.transaction, tenant_id, source_file_id)
            .await
            .map(|file| file.map(source_record))
    }

    async fn restore_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .restore_import_file_for_reference_in_txn(
                &self.transaction,
                tenant_id,
                source_file_id,
                now,
            )
            .await
    }

    async fn create(
        &self,
        job: NewUserImportJob,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<UserImportJobRecord> {
        UserImportRepository
            .create_in_txn(
                &self.transaction,
                CreateUserImportJob {
                    id: job.id,
                    tenant_id: job.tenant_id,
                    requester_user_id: job.requester_user_id,
                    background_job_id: job.background_job_id,
                    idempotency_key_hash: job.idempotency_key_hash,
                    source_file_id: job.source_file_id,
                    source_name_snapshot: job.source_name,
                    source_sha256: job.source_sha256,
                },
                now,
            )
            .await
            .map(job_record)
    }

    async fn mark_source_for_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        FileRepository
            .mark_import_orphan_for_cleanup_in_txn(
                &self.transaction,
                tenant_id,
                source_file_id,
                now,
                cleanup_after,
            )
            .await
    }

    async fn lock_configuration(
        &self,
        import_id: i64,
    ) -> ryframe_kernel::AppResult<Option<String>> {
        let Some(import) = user_import_job::Entity::find_by_id(import_id)
            .one(&self.transaction)
            .await
            .db()?
        else {
            return Ok(None);
        };
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, &import.tenant_id, None)
            .await?;
        Ok(Some(import.tenant_id))
    }

    async fn lock_authorization<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_user_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<UserImportAuthorizationSnapshot> {
        let tenant = TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await?;
        let requester = UserRepository
            .find_by_id_for_update(&self.transaction, tenant_id, requester_user_id)
            .await?;
        Ok(UserImportAuthorizationSnapshot {
            tenant_epoch: tenant.authorization_epoch,
            tenant_available: tenant.is_available(now),
            requester_enabled: requester.as_ref().is_some_and(user::Model::is_enabled),
            requester_version: requester.map(|user| user.authorization_version),
        })
    }

    async fn existing_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        usernames: &'a [String],
    ) -> ryframe_kernel::AppResult<Vec<String>> {
        UserRepository
            .find_existing_usernames_in_txn(&self.transaction, tenant_id, usernames)
            .await
    }

    async fn ensure_user_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_users: usize,
    ) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .ensure_user_quota_for_batch_in_txn(&self.transaction, tenant_id, additional_users)
            .await
    }

    async fn insert_users<'a>(
        &'a self,
        tenant_id: &'a str,
        users: Vec<NewImportedUser>,
    ) -> ryframe_kernel::AppResult<()> {
        UserRepository
            .insert_many_in_txn(
                &self.transaction,
                tenant_id,
                users.into_iter().map(user_model).collect(),
            )
            .await
    }

    async fn insert_rows(&self, rows: Vec<NewUserImportRow>) -> ryframe_kernel::AppResult<()> {
        UserImportRepository
            .insert_row_results_in_txn(
                &self.transaction,
                rows.into_iter().map(import_row_model).collect(),
            )
            .await
    }

    async fn lock(&self, import_id: i64) -> ryframe_kernel::AppResult<Option<UserImportJobRecord>> {
        UserImportRepository
            .lock_by_id_in_txn(&self.transaction, import_id)
            .await
            .map(|job| job.map(job_record))
    }

    async fn save(
        &self,
        record: UserImportJobRecord,
    ) -> ryframe_kernel::AppResult<UserImportJobRecord> {
        UserImportRepository
            .save_in_txn(&self.transaction, job_model(record))
            .await
            .map(job_record)
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseUserImportTransaction {
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
