use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplyRecordDisposition {
    IgnoreReplaced,
    ResyncCommitted,
    Execute,
}

impl TenantConfigTransferService {
    pub(super) async fn prepare_apply(
        &self,
        job: &ClaimedBackgroundJob,
    ) -> AppResult<ApplyDispatch> {
        let tenant_id = job_tenant(job)?.to_owned();
        let transfer_id = payload_id(job, "transfer_id")?;
        let transfer = self
            .persistence
            .find_transfer(&tenant_id, transfer_id)
            .await?
            .ok_or_else(|| AppError::NotFound("配置迁移不存在".into()))?;
        match apply_record_disposition(&transfer, job.id) {
            ApplyRecordDisposition::IgnoreReplaced => return Ok(ApplyDispatch::Completed),
            ApplyRecordDisposition::ResyncCommitted => {
                self.sync_committed_cache_state(&tenant_id, &transfer)
                    .await?;
                return Ok(ApplyDispatch::Completed);
            }
            ApplyRecordDisposition::Execute => {}
        }
        let requester = self
            .user
            .resolve_current_authorization(
                "system",
                transfer.requested_by,
                TRANSFER_APPLY_PERMISSION,
            )
            .await?;
        let owner_token = Uuid::new_v4().to_string();
        let mut lease = self
            .acquire_operation_lease(&tenant_id, transfer_id, &owner_token, "tenant_config.apply")
            .await?;
        if let Err(error) = self
            .mark_transfer_running(
                &tenant_id,
                transfer_id,
                job.id,
                TENANT_CONFIG_APPLY_JOB_TYPE,
                Some(&owner_token),
            )
            .await
        {
            let _ = lease.release().await;
            return Err(error);
        }
        let package = match self
            .load_bundle_package(&tenant_id, transfer.bundle_id)
            .await
        {
            Ok(package) => package,
            Err(error) => {
                let _ = lease.release().await;
                return Err(error);
            }
        };
        Ok(ApplyDispatch::Ready(Box::new(PreparedApply {
            tenant_id,
            transfer_id,
            job_id: job.id,
            owner_token,
            transfer,
            requester,
            package,
            lease,
            phase: ApplyLifecyclePhase::LeaseHeld,
        })))
    }
}

fn apply_record_disposition(
    transfer: &TenantConfigTransferRecord,
    job_id: i64,
) -> ApplyRecordDisposition {
    if transfer.apply_background_job_id != Some(job_id) {
        ApplyRecordDisposition::IgnoreReplaced
    } else if transfer.status == TenantConfigTransferRecord::STATUS_APPLIED {
        ApplyRecordDisposition::ResyncCommitted
    } else {
        ApplyRecordDisposition::Execute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_task_is_ignored_before_acquiring_resources() {
        let transfer = transfer(Some(41), TenantConfigTransferRecord::STATUS_APPLY_PENDING);

        assert_eq!(
            apply_record_disposition(&transfer, 42),
            ApplyRecordDisposition::IgnoreReplaced
        );
    }

    #[test]
    fn committed_task_retries_only_post_commit_cache_sync() {
        let transfer = transfer(Some(42), TenantConfigTransferRecord::STATUS_APPLIED);

        assert_eq!(
            apply_record_disposition(&transfer, 42),
            ApplyRecordDisposition::ResyncCommitted
        );
    }

    #[test]
    fn pending_current_task_enters_apply_workflow() {
        let transfer = transfer(Some(42), TenantConfigTransferRecord::STATUS_APPLY_PENDING);

        assert_eq!(
            apply_record_disposition(&transfer, 42),
            ApplyRecordDisposition::Execute
        );
    }

    fn transfer(job_id: Option<i64>, status: &str) -> TenantConfigTransferRecord {
        let now = Utc::now();
        TenantConfigTransferRecord {
            id: 1,
            tenant_id: "tenant-a".to_owned(),
            bundle_id: 2,
            idempotency_key_hash: "idempotency".to_owned(),
            request_kind: "upload".to_owned(),
            request_fingerprint: "fingerprint".to_owned(),
            status: status.to_owned(),
            target_configuration_version: 11,
            target_authorization_epoch: 20,
            plan_hash: Some("plan".to_owned()),
            preview_calculated_at: Some(now),
            preview_background_job_id: Some(40),
            apply_background_job_id: job_id,
            rollback_background_job_id: None,
            snapshot_file_id: None,
            applied_configuration_version: None,
            applied_authorization_epoch: None,
            change_counts: Value::Null,
            error_summary: None,
            requested_by: 3,
            rollback_expires_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}
