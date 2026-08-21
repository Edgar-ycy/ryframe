use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sha2::{Digest, Sha256};

use crate::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind, FILE_DEL_FLAG_NORMAL,
    FILE_UPLOAD_STATUS_CLEANUP, FILE_UPLOAD_STATUS_PENDING, FileCleanupPersistencePort,
    FileCleanupRecord, FileUploadCommitMode, FileUploadRecord,
};

use super::{
    FileService, UploadResponse, map_storage_read_error, map_storage_write_error, run_blocking_task,
};

const RESERVATION_TTL_MINUTES: i64 = 5;
const LEASE_HEARTBEAT_SECONDS: u64 = 30;
const MIN_CLEANUP_GRACE_SECONDS: i64 = 300;
const STALE_RESERVATION_BATCH_SIZE: u64 = 32;
const STALE_CONFIG_PACKAGE_BATCH_SIZE: u64 = 32;
// 配置允许的后台任务最长运行时间为 24 小时。超过该窗口仍无任何持久化引用的
// ready 文件才可能是进程取消遗留物，额外一小时用于覆盖任务终态同步和时钟抖动。
const CONFIG_PACKAGE_ORPHAN_AGE_HOURS: i64 = 25;
const JANITOR_SUCCESS_INTERVAL_SECONDS: u64 = 60;
const JANITOR_INITIAL_ERROR_BACKOFF_SECONDS: u64 = 5;
const JANITOR_MAX_ERROR_BACKOFF_SECONDS: u64 = 300;
const CLEANUP_RETRY_BACKOFF_SECONDS: i64 = 60;
const CLEANUP_CLAIM_SECONDS: i64 = 300;

pub(super) fn reservation_expires_at(now: DateTime<Utc>) -> DateTime<Utc> {
    now + chrono::Duration::minutes(RESERVATION_TTL_MINUTES)
}

fn cleanup_grace(storage: &dyn ArtifactStore) -> chrono::Duration {
    cleanup_grace_for_bound(storage.late_put_completion_bound())
}

fn cleanup_grace_for_bound(late_completion_bound: Duration) -> chrono::Duration {
    let late_completion_seconds =
        i64::try_from(late_completion_bound.as_secs()).unwrap_or(i64::MAX / 2);
    chrono::Duration::seconds(
        MIN_CLEANUP_GRACE_SECONDS.max(late_completion_seconds.saturating_mul(2)),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpiredReservationPlan {
    BeginCleanup { cleanup_after: DateTime<Utc> },
    DeleteCleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompensationPlan {
    DeleteOwnedObject,
    PreserveObject,
}

fn plan_expired_reservation(
    reservation: &FileCleanupRecord,
    now: DateTime<Utc>,
    cleanup_grace: chrono::Duration,
) -> Option<ExpiredReservationPlan> {
    if reservation.del_flag != FILE_DEL_FLAG_NORMAL
        || !reservation
            .reservation_expires_at
            .is_some_and(|expires_at| expires_at <= now)
    {
        return None;
    }
    match reservation.upload_status.as_str() {
        FILE_UPLOAD_STATUS_PENDING => Some(ExpiredReservationPlan::BeginCleanup {
            cleanup_after: now + cleanup_grace,
        }),
        FILE_UPLOAD_STATUS_CLEANUP => Some(ExpiredReservationPlan::DeleteCleanup),
        _ => None,
    }
}

fn plan_compensation(cleanup_claimed: bool) -> CompensationPlan {
    if cleanup_claimed {
        CompensationPlan::DeleteOwnedObject
    } else {
        CompensationPlan::PreserveObject
    }
}

pub(super) enum ReservationOutcome {
    Ready(FileUploadRecord),
    InProgress(FileUploadRecord),
    Reserved(FileUploadRecord),
}

enum ReservationTransactionOutcome {
    Ready(FileUploadRecord),
    Restored(FileUploadRecord),
    InProgress(FileUploadRecord),
    Reserved(FileUploadRecord),
}

/// 在上传预留变为 `ready` 前持有其持久化所有权。
///
/// `Drop` 仅安排尽力而为的快速清理。正确的取消与崩溃恢复依赖持久化的
/// `pending`/`cleanup` 记录及其 TTL；即使本进程从未运行 `Drop`，全局清理器
/// 也会协调处理这些记录。
pub(super) struct UploadReservationGuard {
    cleanup: Arc<dyn FileCleanupPersistencePort>,
    storage: Arc<dyn ArtifactStore>,
    reservation: Option<FileUploadRecord>,
}

impl UploadReservationGuard {
    pub(super) fn new(
        cleanup: Arc<dyn FileCleanupPersistencePort>,
        storage: Arc<dyn ArtifactStore>,
        reservation: FileUploadRecord,
    ) -> Self {
        Self {
            cleanup,
            storage,
            reservation: Some(reservation),
        }
    }

    pub(super) fn reservation(&self) -> &FileUploadRecord {
        self.reservation
            .as_ref()
            .expect("upload reservation guard must be armed")
    }

    pub(super) fn disarm(&mut self) {
        self.reservation = None;
    }

    pub(super) async fn compensate(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            compensate_upload_reservation(
                Arc::clone(&self.cleanup),
                Arc::clone(&self.storage),
                reservation,
            )
            .await;
        }
    }
}

impl Drop for UploadReservationGuard {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        let cleanup = Arc::clone(&self.cleanup);
        let storage = Arc::clone(&self.storage);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                drop(handle.spawn(async move {
                    compensate_upload_reservation(cleanup, storage, reservation).await;
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

        // 模型时间戳在等待租户锁之前准备。紧接 PUT 前从主数据库时钟续期一次，
        // 避免过长的锁等待使新预留过期。
        let database_now = self.uploads.database_now().await?;
        if !self
            .uploads
            .renew_pending(
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
        // `interval` 会立即触发一次；消耗该触发，以便仅在第一个心跳间隔后续期。
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
                    let database_now = self.uploads.database_now().await?;
                    let renewed = self
                        .uploads
                        .renew_pending(
                            &reservation.tenant_id,
                            reservation.id,
                            reservation_token,
                            reservation_expires_at(database_now),
                        )
                        .await?;
                    if !renewed {
                        // 丢弃 PUT future 会取消客户端操作。持久化墓碑仍会覆盖清理
                        // 完成前的后端延迟完成情况。
                        return Err(AppError::Conflict("文件上传预留已失效".into()));
                    }
                }
            }
        }
    }

    pub(super) async fn reserve_upload(
        &self,
        tenant_id: &str,
        mut model: FileUploadRecord,
    ) -> AppResult<ReservationOutcome> {
        let transaction = self.uploads.begin().await?;
        let operation = async {
            transaction.lock_tenant(tenant_id).await?;
            let database_now = transaction.database_now().await?;
            model.reservation_expires_at = Some(reservation_expires_at(database_now));
            model.updated_at = database_now;

            let file_sha256 = model.file_sha256.as_str();
            if let Some(existing) = transaction
                .find_by_sha256_for_update(tenant_id, &model.bucket, file_sha256)
                .await?
            {
                if existing.upload_status == crate::FILE_UPLOAD_STATUS_READY {
                    return Ok(ReservationTransactionOutcome::Ready(existing));
                }
                if existing.upload_status == FILE_UPLOAD_STATUS_CLEANUP
                    && existing.reservation_token.is_none()
                    && transaction
                        .restore_for_reference(tenant_id, existing.id, &model.bucket, database_now)
                        .await?
                {
                    let mut restored = existing;
                    restored.upload_status = crate::FILE_UPLOAD_STATUS_READY.to_owned();
                    restored.reservation_token = None;
                    restored.reservation_expires_at = None;
                    restored.updated_at = database_now;
                    return Ok(ReservationTransactionOutcome::Restored(restored));
                }
                return Ok(ReservationTransactionOutcome::InProgress(existing));
            }

            transaction
                .ensure_storage_quota(
                    tenant_id,
                    u64::try_from(model.file_size).unwrap_or_default(),
                )
                .await?;
            transaction
                .insert(tenant_id, model)
                .await
                .map(ReservationTransactionOutcome::Reserved)
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
            ReservationTransactionOutcome::Ready(existing) => {
                if let Err(error) = transaction.rollback().await {
                    // 此分支只读，回滚只用于释放锁；已有的已提交记录仍是权威状态。
                    tracing::warn!(
                        file_id = existing.id,
                        %error,
                        "read-only upload dedupe transaction rollback failed"
                    );
                }
                Ok(ReservationOutcome::Ready(existing))
            }
            ReservationTransactionOutcome::Restored(restored) => {
                // 该分支包含状态写入，必须提交；不能与只读去重的 `Ready` 分支合并回滚。
                match transaction.commit(FileUploadCommitMode::Unbound).await {
                    Ok(()) => Ok(ReservationOutcome::Ready(restored)),
                    Err(commit_error) => {
                        match self.uploads.find_any(tenant_id, restored.id).await {
                            Ok(Some(confirmed))
                                if confirmed.upload_status == crate::FILE_UPLOAD_STATUS_READY
                                    && confirmed.reservation_token.is_none() =>
                            {
                                tracing::warn!(
                                    file_id = restored.id,
                                    %commit_error,
                                    "文件恢复提交响应丢失，但已确认文件可安全复用"
                                );
                                Ok(ReservationOutcome::Ready(confirmed))
                            }
                            Ok(_) => Err(AppError::Database(format!(
                                "文件恢复提交结果未知: {commit_error}"
                            ))),
                            Err(verification_error) => {
                                tracing::error!(
                                    file_id = restored.id,
                                    %commit_error,
                                    %verification_error,
                                    "无法核验结果不明确的文件恢复提交"
                                );
                                Err(AppError::Database(format!(
                                    "文件恢复提交结果未知: {commit_error}"
                                )))
                            }
                        }
                    }
                }
            }
            ReservationTransactionOutcome::InProgress(existing) => {
                if let Err(error) = transaction.rollback().await {
                    tracing::warn!(
                        file_id = existing.id,
                        %error,
                        "read-only in-progress upload transaction rollback failed"
                    );
                }
                Ok(ReservationOutcome::InProgress(existing))
            }
            ReservationTransactionOutcome::Reserved(saved) => {
                match transaction.commit(FileUploadCommitMode::Unbound).await {
                    Ok(()) => Ok(ReservationOutcome::Reserved(saved)),
                    Err(commit_error) => {
                        // 丢失 COMMIT 响应时结果不明确。尚未写入对象，因此需在 PUT 前
                        // 校验持久化所有权。
                        match self.uploads.find_any(tenant_id, saved.id).await {
                            Ok(Some(confirmed))
                                if confirmed.reservation_token == saved.reservation_token
                                    && confirmed.upload_status == FILE_UPLOAD_STATUS_PENDING =>
                            {
                                tracing::warn!(
                                    file_id = saved.id,
                                    %commit_error,
                                    "upload reservation commit response was lost, but ownership was confirmed"
                                );
                                Ok(ReservationOutcome::Reserved(confirmed))
                            }
                            Ok(Some(confirmed))
                                if confirmed.upload_status == crate::FILE_UPLOAD_STATUS_READY =>
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
                }
            }
        }
    }

    pub(super) async fn recover_in_progress_upload(
        &self,
        mut existing: FileUploadRecord,
        expected_sha256: &str,
        commit_mode: FileUploadCommitMode,
    ) -> AppResult<UploadResponse> {
        if existing.upload_status == FILE_UPLOAD_STATUS_CLEANUP {
            return Err(AppError::Conflict(
                "相同文件正在执行失败补偿，请稍后重试".into(),
            ));
        }
        if existing.upload_status != FILE_UPLOAD_STATUS_PENDING {
            return Err(AppError::Internal("文件上传预留状态无效".into()));
        }
        let reservation_token = existing
            .reservation_token
            .as_deref()
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
        let actual_sha256 = run_blocking_task("pending upload verification", move || {
            hex::encode(Sha256::digest(&object))
        })
        .await?;
        let stored_sha256 = existing.file_sha256.as_str();
        if actual_sha256 != expected_sha256
            || stored_sha256 != actual_sha256
            || u64::try_from(existing.file_size).unwrap_or_default()
                != u64::try_from(object_len).unwrap_or_default()
        {
            return Err(AppError::Conflict("相同文件正在上传，请稍后重试".into()));
        }

        let transaction = self.uploads.begin().await?;
        match transaction
            .mark_ready(
                &existing.tenant_id,
                existing.id,
                reservation_token,
                Utc::now(),
            )
            .await
        {
            Ok(true) => match transaction.commit(commit_mode).await {
                Ok(()) => {
                    existing.upload_status = crate::FILE_UPLOAD_STATUS_READY.to_owned();
                    existing.reservation_token = None;
                    existing.reservation_expires_at = None;
                    existing.del_flag = FILE_DEL_FLAG_NORMAL.to_owned();
                    Ok(Self::upload_response_for_existing(existing))
                }
                Err(error) => {
                    if let Some(ready) = self
                        .uploads
                        .find_ready(&existing.tenant_id, existing.id)
                        .await?
                    {
                        tracing::warn!(
                            file_id = ready.id,
                            %error,
                            "upload recovery commit response was ambiguous, but ready state was confirmed"
                        );
                        Ok(Self::upload_response_for_existing(ready))
                    } else {
                        Err(error)
                    }
                }
            },
            Ok(false) => {
                let _ = transaction.rollback().await;
                self.uploads
                    .find_ready(&existing.tenant_id, existing.id)
                    .await?
                    .map_or_else(
                        || Err(AppError::Conflict("相同文件正在上传，请稍后重试".into())),
                        |ready| Ok(Self::upload_response_for_existing(ready)),
                    )
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                if let Some(ready) = self
                    .uploads
                    .find_ready(&existing.tenant_id, existing.id)
                    .await?
                {
                    tracing::warn!(
                        file_id = ready.id,
                        "upload recovery response was ambiguous, but ready state was confirmed"
                    );
                    Ok(Self::upload_response_for_existing(ready))
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(super) async fn finalize_upload(
        &self,
        guard: &mut UploadReservationGuard,
        commit_mode: FileUploadCommitMode,
    ) -> AppResult<()> {
        let reservation = guard.reservation();
        let reservation_token = reservation
            .reservation_token
            .as_deref()
            .ok_or_else(|| AppError::Internal("文件上传预留缺少所有权令牌".into()))?;
        let transaction = self.uploads.begin().await?;
        let result = transaction
            .mark_ready(
                &reservation.tenant_id,
                reservation.id,
                reservation_token,
                Utc::now(),
            )
            .await;
        match result {
            Ok(true) => match transaction.commit(commit_mode).await {
                Ok(()) => Ok(()),
                Err(error) => match self
                    .uploads
                    .find_ready(&reservation.tenant_id, reservation.id)
                    .await
                {
                    Ok(Some(_)) => {
                        tracing::warn!(
                            file_id = reservation.id,
                            %error,
                            "upload finalization commit response was ambiguous, but ready state was confirmed"
                        );
                        Ok(())
                    }
                    Ok(None) => Err(error),
                    Err(verification_error) => {
                        tracing::error!(
                            file_id = reservation.id,
                            %error,
                            %verification_error,
                            "could not verify an ambiguous upload finalization commit"
                        );
                        Err(error)
                    }
                },
            },
            Ok(false) => {
                let _ = transaction.rollback().await;
                if self
                    .uploads
                    .find_ready(&reservation.tenant_id, reservation.id)
                    .await?
                    .is_some()
                {
                    Ok(())
                } else {
                    Err(AppError::Conflict("文件上传预留已失效".into()))
                }
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                match self
                    .uploads
                    .find_ready(&reservation.tenant_id, reservation.id)
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
                }
            }
        }
    }

    /// 启动进程级、有界的上传协调循环。
    pub fn spawn_upload_janitor(self: &Arc<Self>) {
        let service = Arc::clone(self);
        drop(tokio::spawn(async move {
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

    /// 协调一个全局有界批次。该接口为启动引导、运维修复命令和受控演练公开；
    /// 常规上传不会在对延迟敏感的路径上执行对象删除。
    pub async fn reconcile_upload_reservations(&self) -> AppResult<u64> {
        let now = self.cleanup.database_now().await?;
        let stale_config_packages = self
            .cleanup
            .find_stale_config_packages(
                now - chrono::Duration::hours(CONFIG_PACKAGE_ORPHAN_AGE_HOURS),
                STALE_CONFIG_PACKAGE_BATCH_SIZE,
            )
            .await?;
        let mut processed = 0_u64;
        for file in stale_config_packages {
            if self
                .schedule_unreferenced_config_package_cleanup(&file.tenant_id, file.id)
                .await?
            {
                processed = processed.saturating_add(1);
            }
        }
        let reservations = self
            .cleanup
            .find_expired_reservations(now, STALE_RESERVATION_BATCH_SIZE)
            .await?;
        for reservation in reservations {
            let Some(plan) =
                plan_expired_reservation(&reservation, now, cleanup_grace(self.storage.as_ref()))
            else {
                continue;
            };
            if let ExpiredReservationPlan::BeginCleanup { cleanup_after } = plan {
                // 首次处理仅创建带有新宽限期的墓碑。客户端任务取消后，延迟的 PUT
                // 仍可能完成，因此有意延后删除。
                if self
                    .cleanup
                    .begin_expired_cleanup(
                        &reservation.tenant_id,
                        reservation.id,
                        now,
                        cleanup_after,
                    )
                    .await?
                {
                    processed += 1;
                }
                continue;
            }
            let claimed_at = self.cleanup.database_now().await?;
            let claim_token = uuid::Uuid::new_v4().to_string();
            if !self
                .cleanup
                .claim_expired_cleanup(
                    &reservation.tenant_id,
                    reservation.id,
                    &claim_token,
                    claimed_at,
                    claimed_at + chrono::Duration::seconds(CLEANUP_CLAIM_SECONDS),
                )
                .await?
            {
                continue;
            }
            let delete_result = self
                .storage
                .delete(&reservation.bucket, &reservation.storage_path)
                .await;
            if let Err(error) = delete_result.as_ref()
                && !storage_error_is_not_found(error)
            {
                tracing::error!(
                    file_id = reservation.id,
                    bucket = reservation.bucket,
                    object_key = reservation.storage_path,
                    %error,
                    "expired upload cleanup failed; durable cleanup state was retained"
                );
                let retry_at = self.cleanup.database_now().await?;
                self.cleanup
                    .defer_claim(
                        &reservation.tenant_id,
                        reservation.id,
                        &claim_token,
                        retry_at,
                        retry_at + chrono::Duration::seconds(CLEANUP_RETRY_BACKOFF_SECONDS),
                    )
                    .await?;
                continue;
            }
            if self
                .cleanup
                .complete_claim(&reservation.tenant_id, reservation.id, &claim_token)
                .await?
            {
                processed += 1;
            }
        }
        Ok(processed)
    }
}

async fn compensate_upload_reservation(
    cleanup: Arc<dyn FileCleanupPersistencePort>,
    storage: Arc<dyn ArtifactStore>,
    reservation: FileUploadRecord,
) {
    let Some(reservation_token) = reservation.reservation_token.as_deref() else {
        tracing::error!(
            file_id = reservation.id,
            "cannot compensate an upload reservation without its ownership token"
        );
        return;
    };
    let database_now = match cleanup.database_now().await {
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
    match cleanup
        .begin_owned_cleanup(
            &reservation.tenant_id,
            reservation.id,
            reservation_token,
            cleanup_after,
        )
        .await
    {
        Ok(cleanup_claimed) => match plan_compensation(cleanup_claimed) {
            CompensationPlan::DeleteOwnedObject => {
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
            CompensationPlan::PreserveObject => {
                // 成功完成的一方赢得比较并设置竞争。除非此预留仍拥有该记录，
                // 否则绝不删除对象。
                tracing::debug!(
                    file_id = reservation.id,
                    "upload reservation no longer owns the metadata row; compensation skipped"
                );
            }
        },
        Err(error) => {
            // 有意保留持久化 pending 记录。全局清理器会在 TTL 之后重试协调。
            tracing::error!(
                file_id = reservation.id,
                %error,
                "could not persist upload compensation state"
            );
        }
    }
}

pub(super) fn storage_error_is_not_found(error: &ArtifactStoreError) -> bool {
    error.kind() == ArtifactStoreErrorKind::NotFound
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{ExpiredReservationPlan, plan_expired_reservation};
    use crate::{
        FILE_DEL_FLAG_NORMAL, FILE_UPLOAD_STATUS_CLEANUP, FILE_UPLOAD_STATUS_PENDING,
        FileCleanupRecord,
    };

    fn reservation(status: &str, expires_at: chrono::DateTime<Utc>) -> FileCleanupRecord {
        FileCleanupRecord {
            id: 1,
            tenant_id: "tenant-a".to_owned(),
            bucket: "uploads".to_owned(),
            storage_path: "tenant-a/object".to_owned(),
            upload_status: status.to_owned(),
            reservation_token: Some("token".to_owned()),
            reservation_expires_at: Some(expires_at),
            del_flag: FILE_DEL_FLAG_NORMAL.to_owned(),
        }
    }

    #[test]
    fn expired_pending_reservation_enters_cleanup_grace() {
        let now = Utc::now();
        let grace = Duration::minutes(5);
        let record = reservation(FILE_UPLOAD_STATUS_PENDING, now - Duration::seconds(1));

        assert_eq!(
            plan_expired_reservation(&record, now, grace),
            Some(ExpiredReservationPlan::BeginCleanup {
                cleanup_after: now + grace,
            })
        );
    }

    #[test]
    fn expired_cleanup_reservation_is_ready_for_deletion() {
        let now = Utc::now();
        let record = reservation(FILE_UPLOAD_STATUS_CLEANUP, now - Duration::seconds(1));

        assert_eq!(
            plan_expired_reservation(&record, now, Duration::minutes(5)),
            Some(ExpiredReservationPlan::DeleteCleanup)
        );
    }

    #[test]
    fn active_or_deleted_reservation_is_ignored() {
        let now = Utc::now();
        let active = reservation(FILE_UPLOAD_STATUS_PENDING, now + Duration::seconds(1));
        assert_eq!(
            plan_expired_reservation(&active, now, Duration::minutes(5)),
            None
        );

        let mut deleted = reservation(FILE_UPLOAD_STATUS_CLEANUP, now - Duration::seconds(1));
        deleted.del_flag = "1".to_owned();
        assert_eq!(
            plan_expired_reservation(&deleted, now, Duration::minutes(5)),
            None
        );
    }
}
