use super::*;

impl DataRetentionService {
    pub(super) fn import_artifact_cutoff(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now - Duration::hours(i64::from(self.config.user_import_artifact_hours))
    }

    pub(super) async fn cleanup_import_artifacts(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<RetentionCleanupResult> {
        let before = self.import_artifact_cutoff(now);
        let maximum = self.config.max_rows_per_resource_per_run;
        let mut deleted = 0_u64;
        let mut after_id = None;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = self.config.cleanup_batch_size.min(remaining_limit);
            if limit == 0 {
                break;
            }
            let artifacts = UserImportRepository
                .list_expired_artifacts_after_id(self.db.write(), before, after_id, limit)
                .await?;
            if artifacts.is_empty() {
                break;
            }
            let batch_len = artifacts.len();
            for artifact in artifacts {
                after_id = Some(artifact.file_id);
                if self
                    .file_service
                    .delete_expired_import_artifact(&artifact.tenant_id, artifact.file_id, before)
                    .await?
                {
                    deleted = deleted.saturating_add(1);
                }
            }
            if batch_len < limit {
                break;
            }
        }
        let remaining = UserImportRepository
            .count_expired_artifacts(self.db.write(), before)
            .await?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }
}
