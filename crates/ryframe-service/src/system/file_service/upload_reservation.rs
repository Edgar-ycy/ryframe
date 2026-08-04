use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use ryframe_core::repository::Repository;
use ryframe_db::{DatabaseCluster, FileRepository, entities::sys_file};
use ryframe_kernel::{AppError, AppResult};
use ryframe_storage::{ObjectStorage, StorageError};
use sea_orm::TransactionTrait;
use sha2::{Digest, Sha256};

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
    reservation: &sys_file::Model,
    now: DateTime<Utc>,
    cleanup_grace: chrono::Duration,
) -> Option<ExpiredReservationPlan> {
    if reservation.del_flag != sys_file::Model::DEL_FLAG_NORMAL
        || !reservation
            .reservation_expires_at
            .is_some_and(|expires_at| expires_at <= now)
    {
        return None;
    }
    match reservation.upload_status.as_str() {
        sys_file::Model::UPLOAD_STATUS_PENDING => Some(ExpiredReservationPlan::BeginCleanup {
            cleanup_after: now + cleanup_grace,
        }),
        sys_file::Model::UPLOAD_STATUS_CLEANUP => Some(ExpiredReservationPlan::DeleteCleanup),
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
    Ready(sys_file::Model),
    InProgress(sys_file::Model),
    Reserved(sys_file::Model),
}

/// 在上传预留变为 `ready` 前持有其持久化所有权。
///
/// `Drop` 仅安排尽力而为的快速清理。正确的取消与崩溃恢复依赖持久化的
/// `pending`/`cleanup` 记录及其 TTL；即使本进程从未运行 `Drop`，全局清理器
/// 也会协调处理这些记录。
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
                drop(handle.spawn(async move {
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

        // 模型时间戳在等待租户锁之前准备。紧接 PUT 前从主数据库时钟续期一次，
        // 避免过长的锁等待使新预留过期。
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

            let file_sha256 = model.file_sha256.as_str();
            if let Some(existing) = FileRepository
                .find_by_sha256_any_status_in_txn(
                    &transaction,
                    tenant_id,
                    &model.bucket,
                    file_sha256,
                )
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
            ReservationOutcome::InProgress(existing) => {
                if let Err(error) = transaction.rollback().await {
                    tracing::warn!(
                        file_id = existing.id,
                        %error,
                        "read-only in-progress upload transaction rollback failed"
                    );
                }
                Ok(ReservationOutcome::InProgress(existing))
            }
            ReservationOutcome::Reserved(saved) => {
                match FileRepository.commit_upload_reservation(transaction).await {
                    Ok(()) => Ok(ReservationOutcome::Reserved(saved)),
                    Err(commit_error) => {
                        // 丢失 COMMIT 响应时结果不明确。尚未写入对象，因此需在 PUT 前
                        // 校验持久化所有权。
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
                                if confirmed.upload_status
                                    == sys_file::Model::UPLOAD_STATUS_READY =>
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
        mut existing: sys_file::Model,
        expected_sha256: &str,
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

        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启上传恢复事务失败: {error}")))?;
        match FileRepository
            .mark_ready(
                &transaction,
                &existing.tenant_id,
                existing.id,
                &reservation_token,
                Utc::now(),
            )
            .await
        {
            Ok(true) => match crate::commit_current_audit(transaction).await {
                Ok(()) => {
                    existing.upload_status = sys_file::Model::UPLOAD_STATUS_READY.to_owned();
                    existing.reservation_token = None;
                    existing.reservation_expires_at = None;
                    existing.del_flag = sys_file::Model::DEL_FLAG_NORMAL.to_owned();
                    Ok(Self::upload_response_for_existing(existing))
                }
                Err(error) => {
                    if let Some(ready) = FileRepository
                        .find_by_id(self.db.write(), &existing.tenant_id, existing.id)
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
                FileRepository
                    .find_by_id(self.db.write(), &existing.tenant_id, existing.id)
                    .await?
                    .map_or_else(
                        || Err(AppError::Conflict("相同文件正在上传，请稍后重试".into())),
                        |ready| Ok(Self::upload_response_for_existing(ready)),
                    )
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                if let Some(ready) = FileRepository
                    .find_by_id(self.db.write(), &existing.tenant_id, existing.id)
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
    ) -> AppResult<()> {
        let reservation = guard.reservation();
        let reservation_token = reservation
            .reservation_token
            .as_deref()
            .ok_or_else(|| AppError::Internal("文件上传预留缺少所有权令牌".into()))?;
        let transaction = self
            .db
            .write()
            .begin()
            .await
            .map_err(|error| AppError::Database(format!("开启上传完成事务失败: {error}")))?;
        let result = FileRepository
            .mark_ready(
                &transaction,
                &reservation.tenant_id,
                reservation.id,
                reservation_token,
                Utc::now(),
            )
            .await;
        match result {
            Ok(true) => match crate::commit_current_audit(transaction).await {
                Ok(()) => Ok(()),
                Err(error) => match FileRepository
                    .find_by_id(self.db.write(), &reservation.tenant_id, reservation.id)
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
            Err(error) => {
                let _ = transaction.rollback().await;
                match FileRepository
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

    /// 协调一个全局有界批次。该接口为启动引导、运维修复命令和集成测试公开；
    /// 常规上传不会在对延迟敏感的路径上执行对象删除。
    pub async fn reconcile_upload_reservations(&self) -> AppResult<u64> {
        let now = FileRepository.database_utc_now(self.db.write()).await?;
        let reservations = FileRepository
            .find_expired_reservations(self.db.write(), now, STALE_RESERVATION_BATCH_SIZE)
            .await?;
        let mut processed = 0_u64;
        for reservation in reservations {
            let Some(plan) =
                plan_expired_reservation(&reservation, now, cleanup_grace(self.storage.as_ref()))
            else {
                continue;
            };
            if let ExpiredReservationPlan::BeginCleanup { cleanup_after } = plan {
                // 首次处理仅创建带有新宽限期的墓碑。客户端任务取消后，延迟的 PUT
                // 仍可能完成，因此有意延后删除。
                if FileRepository
                    .begin_expired_cleanup(
                        self.db.write(),
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

fn storage_error_is_not_found(error: &StorageError) -> bool {
    match error {
        StorageError::Service { status: 404, .. } => true,
        StorageError::Io { source, .. } => source.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompensationPlan, ExpiredReservationPlan, cleanup_grace_for_bound, plan_compensation,
        plan_expired_reservation,
    };
    use chrono::{Duration as ChronoDuration, Utc};
    use ryframe_db::entities::sys_file;
    use std::time::Duration;

    #[test]
    fn failure_compensation_deletes_object_only_for_owned_reservation() {
        assert_eq!(plan_compensation(true), CompensationPlan::DeleteOwnedObject);
        assert_eq!(plan_compensation(false), CompensationPlan::PreserveObject);
    }

    #[test]
    fn expired_cleanup_plan_is_idempotent_and_preserves_two_phase_grace_period() {
        let now = Utc::now();
        let grace = cleanup_grace_for_bound(Duration::from_secs(600));
        assert_eq!(grace, ChronoDuration::seconds(1_200));
        assert_eq!(
            cleanup_grace_for_bound(Duration::from_secs(30)),
            ChronoDuration::seconds(300)
        );

        let mut reservation = reservation(
            sys_file::Model::UPLOAD_STATUS_PENDING,
            Some(now - ChronoDuration::seconds(1)),
        );
        let expected = ExpiredReservationPlan::BeginCleanup {
            cleanup_after: now + grace,
        };
        assert_eq!(
            plan_expired_reservation(&reservation, now, grace),
            Some(expected)
        );
        assert_eq!(
            plan_expired_reservation(&reservation, now, grace),
            Some(expected),
            "同一过期 pending 记录重复规划必须得到相同墓碑截止时间"
        );

        reservation.upload_status = sys_file::Model::UPLOAD_STATUS_CLEANUP.to_owned();
        reservation.reservation_expires_at = Some(now + grace);
        assert_eq!(plan_expired_reservation(&reservation, now, grace), None);
        assert_eq!(
            plan_expired_reservation(&reservation, now + grace, grace),
            Some(ExpiredReservationPlan::DeleteCleanup)
        );
        assert_eq!(
            plan_expired_reservation(&reservation, now + grace, grace),
            Some(ExpiredReservationPlan::DeleteCleanup),
            "到期 cleanup 记录重复规划仍应执行幂等对象删除"
        );
    }

    fn reservation(status: &str, expires_at: Option<chrono::DateTime<Utc>>) -> sys_file::Model {
        let now = Utc::now();
        sys_file::Model {
            id: 1,
            tenant_id: "system".into(),
            original_name: "原始文件.txt".into(),
            storage_name: "opaque.txt".into(),
            storage_path: "system/opaque.txt".into(),
            bucket: "uploads".into(),
            file_url: "uploads/system/opaque.txt".into(),
            file_size: 1,
            content_type: "text/plain".into(),
            file_sha256: "a".repeat(64),
            upload_by: None,
            upload_status: status.into(),
            reservation_token: Some("owner".into()),
            reservation_expires_at: expires_at,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.into(),
            created_at: now,
            updated_at: now,
        }
    }
}
