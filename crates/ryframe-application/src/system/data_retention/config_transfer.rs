use super::*;
use chrono::{DateTime, Utc};

impl DataRetentionService {
    pub(super) async fn preview_tenant_config_artifacts(
        &self,
        now: OffsetDateTime,
    ) -> AppResult<BTreeMap<String, u64>> {
        self.config_artifacts
            .preview(config_artifact_clock(now)?)
            .await
            .map(TenantConfigArtifactCounts::into_resource_counts)
    }

    pub(super) async fn cleanup_tenant_config_artifacts(
        &self,
        now: OffsetDateTime,
    ) -> AppResult<BTreeMap<String, RetentionCleanupResult>> {
        let batch_size = self.config.cleanup_batch_size;
        let maximum = self.config.max_rows_per_resource_per_run;
        let now = config_artifact_clock(now)?;
        let packages = self
            .config_artifacts
            .cleanup_packages(now, batch_size, maximum)
            .await?;
        let snapshots = self
            .config_artifacts
            .cleanup_snapshots(now, batch_size, maximum)
            .await?;
        Ok(BTreeMap::from([
            (TENANT_CONFIG_PACKAGE_RESOURCE.to_owned(), packages),
            (TENANT_CONFIG_SNAPSHOT_RESOURCE.to_owned(), snapshots),
        ]))
    }
}

fn config_artifact_clock(now: OffsetDateTime) -> AppResult<DateTime<Utc>> {
    DateTime::from_timestamp(now.unix_timestamp(), now.nanosecond())
        .ok_or_else(|| AppError::Database("租户配置保留时间无法精确表示为 UTC".into()))
}
