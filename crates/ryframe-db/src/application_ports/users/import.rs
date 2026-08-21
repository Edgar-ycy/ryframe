use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, CreateUserImportJob, DeptRepository, FileRepository,
    TenantConfigTransferRepository, TenantRepository, UserImportFilter, UserImportRepository,
    UserRepository,
    entities::{sys_file, user, user_import_job, user_import_row_result},
};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use sea_orm::{EntityTrait, TransactionTrait};

use super::super::control_transaction::DatabasePortTransaction;

use ryframe_application::{
    EnqueueJob, EnqueueJobResult, PersistenceFuture,
    ports::jobs::BackgroundJobTransaction,
    ports::users::{
        NewImportedUser, NewUserImportJob, NewUserImportRow, UserImportAuthorizationSnapshot,
        UserImportDepartmentRecord, UserImportJobRecord, UserImportPersistencePort,
        UserImportReadFilter, UserImportRowRecord, UserImportSourceRecord, UserImportSourceState,
        UserImportTransaction,
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

impl UserImportPersistencePort for DatabaseUserImportPersistence {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn UserImportTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseUserImportTransaction {
                transaction: transaction.into(),
            }) as Box<dyn UserImportTransaction>)
        })
    }

    fn list_departments<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<UserImportDepartmentRecord>> {
        Box::pin(async move {
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
        })
    }

    fn list<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: UserImportReadFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<UserImportJobRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, Option<UserImportJobRecord>> {
        Box::pin(async move {
            UserImportRepository
                .find_by_id_for_tenant(self.database.write(), tenant_id, import_id)
                .await
                .map(|job| job.map(job_record))
        })
    }

    fn find_global(&self, import_id: i64) -> PersistenceFuture<'_, Option<UserImportJobRecord>> {
        Box::pin(async move {
            user_import_job::Entity::find_by_id(import_id)
                .one(self.database.write())
                .await
                .map(|job| job.map(job_record))
                .map_err(database_error)
        })
    }

    fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<UserImportJobRecord>> {
        Box::pin(async move {
            UserImportRepository
                .find_by_background_job(self.database.write(), background_job_id)
                .await
                .map(|job| job.map(job_record))
        })
    }

    fn rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
        page: ValidatedPageQuery,
    ) -> PersistenceFuture<'a, PageResult<UserImportRowRecord>> {
        Box::pin(async move {
            let result = UserImportRepository
                .list_row_results(self.database.write(), tenant_id, import_id, &page)
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(row_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn requester_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<(i64, String)>> {
        Box::pin(async move {
            UserRepository
                .find_usernames_by_ids(self.database.write(), tenant_id, user_ids)
                .await
        })
    }

    fn all_rows<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, Vec<UserImportRowRecord>> {
        Box::pin(async move {
            UserImportRepository
                .all_row_results(self.database.write(), tenant_id, import_id)
                .await
                .map(|rows| rows.into_iter().map(row_record).collect())
        })
    }

    fn request_cancel<'a>(
        &'a self,
        tenant_id: &'a str,
        import_id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            let now = UserImportRepository
                .database_utc_now(self.database.write())
                .await?;
            UserImportRepository
                .request_cancel(self.database.write(), tenant_id, import_id, now)
                .await
        })
    }
}

impl BackgroundJobTransaction for DatabaseUserImportTransaction {
    fn enqueue(&self, command: EnqueueJob) -> PersistenceFuture<'_, EnqueueJobResult> {
        <DatabasePortTransaction as BackgroundJobTransaction>::enqueue(&self.transaction, command)
    }

    fn reactivate_linked<'a>(
        &'a self,
        job_id: i64,
        expected_job_type: &'a str,
        payload_key: &'a str,
        expected_resource_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        <DatabasePortTransaction as BackgroundJobTransaction>::reactivate_linked(
            &self.transaction,
            job_id,
            expected_job_type,
            payload_key,
            expected_resource_id,
            now,
        )
    }
}

impl UserImportTransaction for DatabaseUserImportTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            UserImportRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantRepository
                .lock_tenant_in_txn(&self.transaction, tenant_id)
                .await
                .map(|_| ())
        })
    }

    fn find_by_idempotency<'a>(
        &'a self,
        tenant_id: &'a str,
        idempotency_key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<UserImportJobRecord>> {
        Box::pin(async move {
            UserImportRepository
                .find_by_idempotency_in_txn(&self.transaction, tenant_id, idempotency_key_hash)
                .await
                .map(|job| job.map(job_record))
        })
    }

    fn requester_username<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<String>> {
        Box::pin(async move {
            UserRepository
                .find_usernames_by_ids(&self.transaction, tenant_id, &[user_id])
                .await
                .map(|users| users.into_iter().next().map(|(_, username)| username))
        })
    }

    fn active_count<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            UserImportRepository
                .count_active_in_txn(&self.transaction, tenant_id)
                .await
        })
    }

    fn lock_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
    ) -> PersistenceFuture<'a, Option<UserImportSourceRecord>> {
        Box::pin(async move {
            FileRepository
                .find_by_id_any_status_for_update(&self.transaction, tenant_id, source_file_id)
                .await
                .map(|file| file.map(source_record))
        })
    }

    fn restore_source<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            FileRepository
                .restore_import_file_for_reference_in_txn(
                    &self.transaction,
                    tenant_id,
                    source_file_id,
                    now,
                )
                .await
        })
    }

    fn create(
        &self,
        job: NewUserImportJob,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, UserImportJobRecord> {
        Box::pin(async move {
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
        })
    }

    fn mark_source_for_cleanup<'a>(
        &'a self,
        tenant_id: &'a str,
        source_file_id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            FileRepository
                .mark_import_orphan_for_cleanup_in_txn(
                    &self.transaction,
                    tenant_id,
                    source_file_id,
                    now,
                    cleanup_after,
                )
                .await
        })
    }

    fn lock_configuration(&self, import_id: i64) -> PersistenceFuture<'_, Option<String>> {
        Box::pin(async move {
            let Some(import) = user_import_job::Entity::find_by_id(import_id)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
            else {
                return Ok(None);
            };
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, &import.tenant_id, None)
                .await?;
            Ok(Some(import.tenant_id))
        })
    }

    fn lock_authorization<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_user_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, UserImportAuthorizationSnapshot> {
        Box::pin(async move {
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
        })
    }

    fn existing_usernames<'a>(
        &'a self,
        tenant_id: &'a str,
        usernames: &'a [String],
    ) -> PersistenceFuture<'a, Vec<String>> {
        Box::pin(async move {
            UserRepository
                .find_existing_usernames_in_txn(&self.transaction, tenant_id, usernames)
                .await
        })
    }

    fn ensure_user_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        additional_users: usize,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantRepository
                .ensure_user_quota_for_batch_in_txn(&self.transaction, tenant_id, additional_users)
                .await
        })
    }

    fn insert_users<'a>(
        &'a self,
        tenant_id: &'a str,
        users: Vec<NewImportedUser>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            UserRepository
                .insert_many_in_txn(
                    &self.transaction,
                    tenant_id,
                    users.into_iter().map(user_model).collect(),
                )
                .await
        })
    }

    fn insert_rows(&self, rows: Vec<NewUserImportRow>) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            UserImportRepository
                .insert_row_results_in_txn(
                    &self.transaction,
                    rows.into_iter().map(import_row_model).collect(),
                )
                .await
        })
    }

    fn lock(&self, import_id: i64) -> PersistenceFuture<'_, Option<UserImportJobRecord>> {
        Box::pin(async move {
            UserImportRepository
                .lock_by_id_in_txn(&self.transaction, import_id)
                .await
                .map(|job| job.map(job_record))
        })
    }

    fn save(&self, record: UserImportJobRecord) -> PersistenceFuture<'_, UserImportJobRecord> {
        Box::pin(async move {
            UserImportRepository
                .save_in_txn(&self.transaction, job_model(record))
                .await
                .map(job_record)
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::super::audit_persistence::commit_current_audit(self.transaction.into_inner())
                .await
        })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn job_record(job: user_import_job::Model) -> UserImportJobRecord {
    UserImportJobRecord {
        id: job.id,
        tenant_id: job.tenant_id,
        requester_user_id: job.requester_user_id,
        background_job_id: job.background_job_id,
        idempotency_key_hash: job.idempotency_key_hash,
        source_file_id: job.source_file_id,
        source_name_snapshot: job.source_name_snapshot,
        source_sha256: job.source_sha256,
        duplicate_policy: job.duplicate_policy,
        status: job.status,
        total_rows: job.total_rows,
        processed_rows: job.processed_rows,
        success_count: job.success_count,
        skipped_count: job.skipped_count,
        failure_count: job.failure_count,
        cancel_requested: job.cancel_requested,
        error_report_file_id: job.error_report_file_id,
        last_error: job.last_error,
        started_at: job.started_at,
        completed_at: job.completed_at,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

fn job_model(job: UserImportJobRecord) -> user_import_job::Model {
    user_import_job::Model {
        id: job.id,
        tenant_id: job.tenant_id,
        requester_user_id: job.requester_user_id,
        background_job_id: job.background_job_id,
        idempotency_key_hash: job.idempotency_key_hash,
        source_file_id: job.source_file_id,
        source_name_snapshot: job.source_name_snapshot,
        source_sha256: job.source_sha256,
        duplicate_policy: job.duplicate_policy,
        status: job.status,
        total_rows: job.total_rows,
        processed_rows: job.processed_rows,
        success_count: job.success_count,
        skipped_count: job.skipped_count,
        failure_count: job.failure_count,
        cancel_requested: job.cancel_requested,
        error_report_file_id: job.error_report_file_id,
        last_error: job.last_error,
        started_at: job.started_at,
        completed_at: job.completed_at,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

fn row_record(row: user_import_row_result::Model) -> UserImportRowRecord {
    UserImportRowRecord {
        row_number: row.row_number,
        username: row.username_snapshot,
        outcome: row.outcome,
        code: row.code,
        message: row.message,
        created_at: row.created_at,
    }
}

fn source_record(file: sys_file::Model) -> UserImportSourceRecord {
    let state = if file.del_flag != sys_file::Model::DEL_FLAG_NORMAL {
        UserImportSourceState::Unavailable
    } else if file.upload_status == sys_file::Model::UPLOAD_STATUS_READY {
        UserImportSourceState::Ready
    } else if file.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP {
        UserImportSourceState::Recoverable
    } else {
        UserImportSourceState::Unavailable
    };
    UserImportSourceRecord {
        bucket: file.bucket,
        sha256: file.file_sha256,
        state,
    }
}

fn user_model(user: NewImportedUser) -> user::Model {
    user::Model {
        id: user.id,
        tenant_id: user.tenant_id,
        username: user.username,
        password_hash: user.password_hash,
        nickname: user.nickname,
        email: user.email,
        phone: user.phone,
        avatar: None,
        avatar_file_id: None,
        preferred_locale: None,
        status: user::Model::STATUS_PENDING_ACTIVATION.to_owned(),
        authorization_version: 1,
        dept_id: Some(user.department_id),
        remark: None,
        login_ip: None,
        login_date: None,
        del_flag: user::Model::DEL_FLAG_NORMAL.to_owned(),
        created_at: user.created_at,
        updated_at: user.created_at,
    }
}

fn import_row_model(row: NewUserImportRow) -> user_import_row_result::Model {
    user_import_row_result::Model {
        id: row.id,
        tenant_id: row.tenant_id,
        import_job_id: row.import_job_id,
        row_number: row.row_number,
        username_snapshot: row.username,
        outcome: row.outcome,
        code: row.code,
        message: row.message,
        created_at: row.created_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
