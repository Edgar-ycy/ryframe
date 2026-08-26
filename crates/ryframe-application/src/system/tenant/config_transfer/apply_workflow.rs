use super::*;

mod preparation;
mod snapshot;
mod transaction;

struct PreparedApply {
    tenant_id: String,
    transfer_id: i64,
    job_id: i64,
    owner_token: String,
    transfer: TenantConfigTransferRecord,
    requester: crate::system::user::CurrentAuthorization,
    package: ParsedTenantConfigPackage,
    lease: super::lifecycle::OperationLease,
    phase: ApplyLifecyclePhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplyLifecyclePhase {
    LeaseHeld,
    SnapshotUploaded,
    TransactionCommitted,
}

enum ApplyDispatch {
    Completed,
    Ready(Box<PreparedApply>),
}

struct RollbackSnapshotSource {
    resources: TenantConfigPackageResources,
    capabilities: Vec<CapabilityRequirement>,
    tenant_name: String,
    generated_at: DateTime<Utc>,
}

struct AppliedCacheVersions {
    authorization_epoch: i32,
    namespace_version: i64,
}

impl PreparedApply {
    fn mark_snapshot_uploaded(&mut self) {
        self.phase = ApplyLifecyclePhase::SnapshotUploaded;
    }

    fn mark_transaction_committed(
        &mut self,
        snapshot: &mut super::lifecycle::RollbackSnapshotFile,
    ) {
        snapshot.retain();
        self.lease.finish();
        self.phase = ApplyLifecyclePhase::TransactionCommitted;
    }

    async fn release_with_error<T>(
        &mut self,
        snapshot: Option<&mut super::lifecycle::RollbackSnapshotFile>,
        error: AppError,
    ) -> AppResult<T> {
        debug_assert_eq!(snapshot.is_some(), self.phase.requires_snapshot_cleanup());
        if let Some(snapshot) = snapshot {
            snapshot.cleanup().await;
        }
        if self.phase.requires_lease_release() {
            let _ = self.lease.release().await;
        }
        Err(error)
    }
}

impl ApplyLifecyclePhase {
    const fn requires_snapshot_cleanup(self) -> bool {
        matches!(self, Self::SnapshotUploaded)
    }

    const fn requires_lease_release(self) -> bool {
        !matches!(self, Self::TransactionCommitted)
    }
}

impl TenantConfigTransferService {
    pub(super) async fn execute_apply(&self, job: &ClaimedBackgroundJob) -> AppResult<()> {
        let ApplyDispatch::Ready(mut prepared) = self.prepare_apply(job).await? else {
            return Ok(());
        };
        let snapshot_source = match self.capture_rollback_snapshot(&prepared).await {
            Ok(source) => source,
            Err(error) => return prepared.release_with_error(None, error).await,
        };
        let mut snapshot = match self
            .build_and_upload_rollback_snapshot(&prepared, snapshot_source)
            .await
        {
            Ok(snapshot) => {
                prepared.mark_snapshot_uploaded();
                snapshot
            }
            Err(error) => return prepared.release_with_error(None, error).await,
        };
        if let Err(error) = self
            .renew_operation_lease(&prepared.tenant_id, &prepared.owner_token)
            .await
        {
            return prepared
                .release_with_error(Some(&mut snapshot), error)
                .await;
        }
        let versions = match self
            .commit_apply_transaction(&prepared, snapshot.file_id())
            .await
        {
            Ok(versions) => versions,
            Err(error) => {
                return prepared
                    .release_with_error(Some(&mut snapshot), error)
                    .await;
            }
        };
        prepared.mark_transaction_committed(&mut snapshot);
        self.sync_applied_cache_state(&prepared.tenant_id, versions)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_build_or_upload_failure_releases_only_lease() {
        let phase = ApplyLifecyclePhase::LeaseHeld;

        assert!(!phase.requires_snapshot_cleanup());
        assert!(phase.requires_lease_release());
    }

    #[test]
    fn renewal_failure_cleans_snapshot_and_releases_lease() {
        assert_pre_commit_cleanup(ApplyLifecyclePhase::SnapshotUploaded);
    }

    #[test]
    fn transaction_or_commit_failure_cleans_snapshot_and_releases_lease() {
        assert_pre_commit_cleanup(ApplyLifecyclePhase::SnapshotUploaded);
    }

    #[test]
    fn cache_sync_failure_keeps_committed_snapshot_and_lease_state() {
        let phase = ApplyLifecyclePhase::TransactionCommitted;

        assert!(!phase.requires_snapshot_cleanup());
        assert!(!phase.requires_lease_release());
    }

    fn assert_pre_commit_cleanup(phase: ApplyLifecyclePhase) {
        assert!(phase.requires_snapshot_cleanup());
        assert!(phase.requires_lease_release());
    }
}
