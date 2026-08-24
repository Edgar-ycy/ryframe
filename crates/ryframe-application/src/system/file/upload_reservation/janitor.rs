use super::*;

impl FileService {
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
