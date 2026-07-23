use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::Repository;
use ryframe_db::{DatabaseCluster, FileRepository, entities::sys_file};
use ryframe_storage::{ObjectStorage, StorageError};
use sea_orm::TransactionTrait;

use super::{
    FileService, UploadResponse, map_storage_read_error, map_storage_write_error, run_blocking_task,
};

const RESERVATION_TTL_MINUTES: i64 = 5;
const LEASE_HEARTBEAT_SECONDS: u64 = 30;
const MIN_CLEANUP_GRACE_SECONDS: i64 = 300;
const STALE_RESERVATION_BATCH_SIZE: u64 = 32;
const JANITOR_SUCCESS_INTERVAL_SECONDS: u64 = 60;
const JANITOR_INITIAL_ERROR_BACKOFF_SECONDS: u64 = 5;
const JANITOR_MAX_ERROR_BACKOFF_SECONDS: u64 = 300;
const CLEANUP_RETRY_BACKOFF_SECONDS: i64 = 60;

pub(super) fn reservation_expires_at(now: DateTime<Utc>) -> DateTime<Utc> {
    now + chrono::Duration::minutes(RESERVATION_TTL_MINUTES)
}

fn cleanup_grace(storage: &dyn ObjectStorage) -> chrono::Duration {
    let late_completion_seconds =
        i64::try_from(storage.late_put_completion_bound().as_secs()).unwrap_or(i64::MAX / 2);
    chrono::Duration::seconds(
        MIN_CLEANUP_GRACE_SECONDS.max(late_completion_seconds.saturating_mul(2)),
    )
}

pub(super) enum ReservationOutcome {
    Ready(sys_file::Model),
    InProgress(sys_file::Model),
    Reserved(sys_file::Model),
}

/// Owns a durable upload reservation until it becomes `ready`.
///
/// `Drop` only schedules a best-effort fast cleanup. Correct cancellation and
/// crash recovery rely on the persisted `pending`/`cleanup` row and its TTL,
/// which the global janitor reconciles even when this process never runs `Drop`.
pub(super) struct UploadReservationGuard {
    db: DatabaseCluster,
    storage: Arc<dyn ObjectStorage>,
    reservation: Option<sys_file::Model>,
}

impl UploadReservationGuard {
    pub(super) fn new(
        db: DatabaseCluster,
        storage: Arc<dyn ObjectStorage>,
        reservation: sys_file::Model,
    ) -> Self {
        Self {
            db,
            storage,
            reservation: Some(reservation),
        }
    }

    pub(super) fn reservation(&self) -> &sys_file::Model {
        self.reservation
            .as_ref()
            .expect("upload reservation guard must be armed")
    }

    pub(super) fn disarm(&mut self) {
        self.reservation = None;
    }

    pub(super) async fn compensate(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            compensate_upload_reservation(self.db.clone(), self.storage.clone(), reservation).await;
        }
    }
}

impl Drop for UploadReservationGuard {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        let db = self.db.clone();
        let storage = self.storage.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                std::mem::drop(handle.spawn(async move {
                    compensate_upload_reservation(db, storage, reservation).await;
                }));
            }
            Err(error) => {
                tracing::error!(
                    file_id = reservation.id,
                    %error,
                    "cannot schedule fast upload cancellation compensation; durable TTL recovery remains armed"
                );
            }
        }
    }
}

impl FileService {
    pub(super) async fn put_reserved_object(
        &self,
        guard: &UploadReservationGuard,
        data: &[u8],
    ) -> AppResult<()> {
        let reservation = guard.reservation();
        let reservation_token = reservation
            .reservation_token
            .as_deref()
            .ok_or_else(|| AppError::Internal("文件上传预留缺少所有权令牌".into()))?;

        // The model timestamp was prepared before waiting for the tenant lock.
        // Renew once from the primary database clock immediately before PUT so
        // a long lock wait cannot make a new reservation stale.
        let database_now = FileRepository.database_utc_now(self.db.write()).await?;
        if !FileRepository
            .renew_pending_reservation(
                self.db.write(),
                &reservation.tenant_id,
                reservation.id,
                reservation_token,
                reservation_expires_at(database_now),
            )
            .await?
        {
            return Err(AppError::Conflict("文件上传预留已失效".into()));
        }

        let mut heartbeat = tokio::time::interval(Duration::from_secs(LEASE_HEARTBEAT_SECONDS));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` ticks immediately once; consume it so renewal happens only
        // after the first heartbeat interval.
        heartbeat.tick().await;
        let put = self.storage.put(
            &reservation.bucket,
            &reservation.storage_path,
            data,
            &reservation.content_type,
        );
        tokio::pin!(put);

        loop {
            tokio::select! {
                result = &mut put => {
                    return result.map_err(|error| {
                        tracing::error!(
                            file_id = reservation.id,
                            bucket = reservation.bucket,
                            object_key = reservation.storage_path,
                            %error,
                            "object storage PUT failed"
                        );
                        map_storage_write_error(error)
                    });
                }
                _ = heartbeat.tick() => {
                    let database_now = FileRepository.database_utc_now(self.db.write()).await?;
                    let renewed = FileRepository
                        .renew_pending_reservation(
                            self.db.write(),
                            &reservation.tenant_id,
                            reservation.id,
                            reservation_token,
                            reservation_expires_at(database_now),
                        )
                        .await?;
                    if !renewed {
                        // Dropping the PUT future cancels the client operation.
                        // The durable tombstone still covers a late backend
                        // completion before cleanup is finalized.
                        return Err(AppError::Conflict("文件上传预留已失效".into()));
                    }
                }
            }
        }
    }

    pub(super) async fn reserve_upload(
        &self,
        tenant_id: &str,
        mut model: sys_file::Model,
    ) -> AppResult<ReservationOutcome> {
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启文件预留事务失败: {error}")))?;
        let operation = async {
            ryframe_db::TenantRepository
                .lock_tenant_in_txn(&transaction, tenant_id)
                .await?;
            let database_now = FileRepository.database_utc_now(&transaction).await?;
            model.reservation_expires_at = Some(reservation_expires_at(database_now));
            model.updated_at = database_now;

            let file_md5 = model
                .file_md5
                .as_deref()
                .ok_or_else(|| AppError::Internal("上传预留缺少内容摘要".into()))?;
            if let Some(existing) = FileRepository
                .find_by_md5_any_status_in_txn(&transaction, tenant_id, &model.bucket, file_md5)
                .await?
            {
                return if existing.upload_status == sys_file::Model::UPLOAD_STATUS_READY {
                    Ok(ReservationOutcome::Ready(existing))
                } else {
                    Ok(ReservationOutcome::InProgress(existing))
                };
            }

            ryframe_db::TenantRepository
                .ensure_storage_quota_in_txn(
                    &transaction,
                    tenant_id,
                    u64::try_from(model.file_size).unwrap_or_default(),
                )
                .await?;
            FileRepository
                .insert_in_txn(&transaction, tenant_id, model)
                .await
                .map(ReservationOutcome::Reserved)
        }
        .await;

        let outcome = match operation {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    tracing::error!(
                        %rollback_error,
                        "upload reservation transaction rollback failed"
                    );
                }
                return Err(error);
            }
        };

        match outcome {
            ReservationOutcome::Ready(existing) => {
                if let Err(error) = transaction.commit().await {
                    // This branch is read-only. The existing committed row is
                    // still authoritative even when releasing the lock loses
                    // its response.
                    tracing::warn!(
                        file_id = existing.id,
                        %error,
                        "read-only upload dedupe transaction commit response was lost"
                    );
                }
                Ok(ReservationOutcome::Ready(existing))
            }
            ReservationOutcome::InProgress(existing) => {
                if let Err(error) = transaction.commit().await {
                    tracing::warn!(
                        file_id = existing.id,
                        %error,
                        "read-only in-progress upload transaction commit response was lost"
                    );
                }
                Ok(ReservationOutcome::InProgress(existing))
            }
            ReservationOutcome::Reserved(saved) => match transaction.commit().await {
                Ok(()) => Ok(ReservationOutcome::Reserved(saved)),
                Err(commit_error) => {
                    // A lost COMMIT response is ambiguous. No object has been
                    // written yet, so verify durable ownership before PUT.
                    match FileRepository
                        .find_by_id_any_status(self.db.write(), tenant_id, saved.id)
                        .await
                    {
                        Ok(Some(confirmed))
                            if confirmed.reservation_token == saved.reservation_token
                                && confirmed.upload_status
                                    == sys_file::Model::UPLOAD_STATUS_PENDING =>
                        {
                            tracing::warn!(
                                file_id = saved.id,
                                %commit_error,
                                "upload reservation commit response was lost, but ownership was confirmed"
                            );
                            Ok(ReservationOutcome::Reserved(confirmed))
                        }
                        Ok(Some(confirmed))
                            if confirmed.upload_status == sys_file::Model::UPLOAD_STATUS_READY =>
                        {
                            Ok(ReservationOutcome::Ready(confirmed))
                        }
                        Ok(_) => Err(AppError::Database(format!(
                            "文件预留提交结果未知: {commit_error}"
                        ))),
                        Err(verification_error) => {
                            tracing::error!(
                                file_id = saved.id,
                                %commit_error,
                                %verification_error,
                                "could not verify an ambiguous upload reservation commit"
                            );
                            Err(AppError::Database(format!(
                                "文件预留提交结果未知: {commit_error}"
                            )))
                        }
                    }
                }
            },
        }
    }

    pub(super) async fn recover_in_progress_upload(
        &self,
        mut existing: sys_file::Model,
        expected_md5: &str,
    ) -> AppResult<UploadResponse> {
        if existing.upload_status == sys_file::Model::UPLOAD_STATUS_CLEANUP {
            return Err(AppError::Conflict(
                "相同文件正在执行失败补偿，请稍后重试".into(),
            ));
        }
        if existing.upload_status != sys_file::Model::UPLOAD_STATUS_PENDING {
            return Err(AppError::Internal("文件上传预留状态无效".into()));
        }
        let reservation_token = existing
            .reservation_token
            .clone()
            .ok_or_else(|| AppError::Internal("文件上传预留缺少所有权令牌".into()))?;

        let object = match self
            .storage
            .get(&existing.bucket, &existing.storage_path)
            .await
        {
            Ok(object) => object,
            Err(error) if storage_error_is_not_found(&error) => {
                return Err(AppError::Conflict("相同文件正在上传，请稍后重试".into()));
            }
            Err(error) => return Err(map_storage_read_error(error)),
        };
        let object_len = object.len();
        let actual_md5 = run_blocking_task("pending upload verification", move || {
            format!("{:x}", md5::compute(object))
        })
        .await?;
        if actual_md5 != expected_md5
            || u64::try_from(existing.file_size).unwrap_or_default()
                != u64::try_from(object_len).unwrap_or_default()
        {
            return Err(AppError::Conflict("相同文件正在上传，请稍后重试".into()));
        }

        match FileRepository
            .mark_ready(
                self.db.write(),
                &existing.tenant_id,
                existing.id,
                &reservation_token,
                Utc::now(),
            )
            .await
        {
            Ok(true) => {
                existing.upload_status = sys_file::Model::UPLOAD_STATUS_READY.to_owned();
                existing.reservation_token = None;
                existing.reservation_expires_at = None;
                existing.del_flag = sys_file::Model::DEL_FLAG_NORMAL.to_owned();
                self.upload_response_for_existing(existing)
            }
            Ok(false) => FileRepository
                .find_by_id(self.db.write(), &existing.tenant_id, existing.id)
                .await?
                .map_or_else(
                    || Err(AppError::Conflict("相同文件正在上传，请稍后重试".into())),
                    |ready| self.upload_response_for_existing(ready),
                ),
            Err(error) => {
                if let Some(ready) = FileRepository
                    .find_by_id(self.db.write(), &existing.tenant_id, existing.id)
                    .await?
                {
                    tracing::warn!(
                        file_id = ready.id,
                        "upload recovery response was ambiguous, but ready state was confirmed"
                    );
                    self.upload_response_for_existing(ready)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(super) async fn finalize_upload(
        &self,
        guard: &mut UploadReservationGuard,
    ) -> AppResult<()> {
        let reservation = guard.reservation();
        let reservation_token = reservation
            .reservation_token
            .as_deref()
            .ok_or_else(|| AppError::Internal("文件上传预留缺少所有权令牌".into()))?;
        let result = FileRepository
            .mark_ready(
                self.db.write(),
                &reservation.tenant_id,
                reservation.id,
                reservation_token,
                Utc::now(),
            )
            .await;
        match result {
            Ok(true) => Ok(()),
            Ok(false) => {
                if FileRepository
                    .find_by_id(self.db.write(), &reservation.tenant_id, reservation.id)
                    .await?
                    .is_some()
                {
                    Ok(())
                } else {
                    Err(AppError::Conflict("文件上传预留已失效".into()))
                }
            }
            Err(error) => match FileRepository
                .find_by_id(self.db.write(), &reservation.tenant_id, reservation.id)
                .await
            {
                Ok(Some(_)) => {
                    tracing::warn!(
                        file_id = reservation.id,
                        %error,
                        "upload finalization response was ambiguous, but ready state was confirmed"
                    );
                    Ok(())
                }
                Ok(None) => Err(error),
                Err(verification_error) => {
                    tracing::error!(
                        file_id = reservation.id,
                        %error,
                        %verification_error,
                        "could not verify an ambiguous upload finalization"
                    );
                    Err(error)
                }
            },
        }
    }

    /// Start the process-wide, bounded upload reconciliation loop.
    pub fn spawn_upload_janitor(self: &Arc<Self>) {
        let service = Arc::clone(self);
        std::mem::drop(tokio::spawn(async move {
            let mut next_delay = Duration::ZERO;
            let mut error_backoff = JANITOR_INITIAL_ERROR_BACKOFF_SECONDS;
            loop {
                tokio::time::sleep(next_delay).await;
                match service.reconcile_upload_reservations().await {
                    Ok(processed) => {
                        if processed > 0 {
                            tracing::info!(processed, "upload reservation janitor batch completed");
                        }
                        next_delay = Duration::from_secs(JANITOR_SUCCESS_INTERVAL_SECONDS);
                        error_backoff = JANITOR_INITIAL_ERROR_BACKOFF_SECONDS;
                    }
                    Err(error) => {
                        tracing::error!(
                            %error,
                            retry_seconds = error_backoff,
                            "upload reservation janitor batch failed"
                        );
                        next_delay = Duration::from_secs(error_backoff);
                        error_backoff = error_backoff
                            .saturating_mul(2)
                            .min(JANITOR_MAX_ERROR_BACKOFF_SECONDS);
                    }
                }
            }
        }));
    }

    /// Reconcile one globally bounded batch. This is public for bootstrapping,
    /// operational repair commands, and integration tests; normal uploads do
    /// not execute object deletions on their latency-sensitive path.
    pub async fn reconcile_upload_reservations(&self) -> AppResult<u64> {
        let now = FileRepository.database_utc_now(self.db.write()).await?;
        let reservations = FileRepository
            .find_expired_reservations(self.db.write(), now, STALE_RESERVATION_BATCH_SIZE)
            .await?;
        let mut processed = 0_u64;
        for reservation in reservations {
            if reservation.upload_status == sys_file::Model::UPLOAD_STATUS_PENDING {
                // First pass only creates a tombstone with a fresh grace
                // window. A late PUT can still complete after its client task
                // was cancelled, so deletion is deliberately deferred.
                if FileRepository
                    .begin_expired_cleanup(
                        self.db.write(),
                        &reservation.tenant_id,
                        reservation.id,
                        now,
                        now + cleanup_grace(self.storage.as_ref()),
                    )
                    .await?
                {
                    processed += 1;
                }
                continue;
            }
            if reservation.upload_status != sys_file::Model::UPLOAD_STATUS_CLEANUP {
                continue;
            }
            if let Err(error) = self
                .storage
                .delete(&reservation.bucket, &reservation.storage_path)
                .await
            {
                tracing::error!(
                    file_id = reservation.id,
                    bucket = reservation.bucket,
                    object_key = reservation.storage_path,
                    %error,
                    "expired upload cleanup failed; durable cleanup state was retained"
                );
                FileRepository
                    .defer_cleanup_retry(
                        self.db.write(),
                        &reservation.tenant_id,
                        reservation.id,
                        now,
                        now + chrono::Duration::seconds(CLEANUP_RETRY_BACKOFF_SECONDS),
                    )
                    .await?;
                continue;
            }
            if FileRepository
                .delete_expired_cleanup(
                    self.db.write(),
                    &reservation.tenant_id,
                    reservation.id,
                    now,
                )
                .await?
            {
                processed += 1;
            }
        }
        Ok(processed)
    }
}

async fn compensate_upload_reservation(
    db: DatabaseCluster,
    storage: Arc<dyn ObjectStorage>,
    reservation: sys_file::Model,
) {
    let Some(reservation_token) = reservation.reservation_token.as_deref() else {
        tracing::error!(
            file_id = reservation.id,
            "cannot compensate an upload reservation without its ownership token"
        );
        return;
    };
    let database_now = match FileRepository.database_utc_now(db.write()).await {
        Ok(now) => now,
        Err(error) => {
            tracing::error!(
                file_id = reservation.id,
                %error,
                "could not read the database clock for upload compensation"
            );
            return;
        }
    };
    let cleanup_after = database_now + cleanup_grace(storage.as_ref());
    match FileRepository
        .begin_cleanup(
            db.write(),
            &reservation.tenant_id,
            reservation.id,
            reservation_token,
            cleanup_after,
        )
        .await
    {
        Ok(true) => {
            if let Err(error) = storage
                .delete(&reservation.bucket, &reservation.storage_path)
                .await
            {
                tracing::error!(
                    file_id = reservation.id,
                    bucket = reservation.bucket,
                    object_key = reservation.storage_path,
                    %error,
                    "upload compensation could not delete the object; the cleanup record was retained"
                );
            }
        }
        Ok(false) => {
            // A successful finalization wins the compare-and-set race. Never
            // delete an object unless this reservation still owns the row.
            tracing::debug!(
                file_id = reservation.id,
                "upload reservation no longer owns the metadata row; compensation skipped"
            );
        }
        Err(error) => {
            // The durable pending record intentionally remains. The global
            // janitor retries reconciliation after the TTL.
            tracing::error!(
                file_id = reservation.id,
                %error,
                "could not persist upload compensation state"
            );
        }
    }
}

fn storage_error_is_not_found(error: &StorageError) -> bool {
    match error {
        StorageError::Service { status: 404, .. } => true,
        StorageError::Io { source, .. } => source.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}
