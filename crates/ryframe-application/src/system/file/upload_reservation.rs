use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sha2::{Digest, Sha256};

use crate::{
    TransactionAuditMode,
    ports::files::{
        ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind, FILE_DEL_FLAG_NORMAL,
        FILE_UPLOAD_STATUS_CLEANUP, FILE_UPLOAD_STATUS_PENDING, FILE_UPLOAD_STATUS_READY,
        FileCleanupPersistencePort, FileCleanupRecord, FileUploadRecord,
    },
};

use super::{
    FileService, UploadResponse, map_storage_read_error, map_storage_write_error, run_blocking_task,
};

mod janitor;
mod operations;

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
pub enum ExpiredReservationPlan {
    BeginCleanup { cleanup_after: DateTime<Utc> },
    DeleteCleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompensationPlan {
    DeleteOwnedObject,
    PreserveObject,
}

pub fn plan_expired_reservation(
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
