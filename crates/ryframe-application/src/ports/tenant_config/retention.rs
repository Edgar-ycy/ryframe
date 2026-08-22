use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::{PersistenceFuture, ports::retention::RetentionCleanupResult};

pub const TENANT_CONFIG_PACKAGE_RESOURCE: &str = "tenant_config_packages";
pub const TENANT_CONFIG_SNAPSHOT_RESOURCE: &str = "tenant_config_snapshots";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantConfigArtifactCounts {
    pub packages: u64,
    pub snapshots: u64,
}

impl TenantConfigArtifactCounts {
    pub fn into_resource_counts(self) -> BTreeMap<String, u64> {
        BTreeMap::from([
            (TENANT_CONFIG_PACKAGE_RESOURCE.to_owned(), self.packages),
            (TENANT_CONFIG_SNAPSHOT_RESOURCE.to_owned(), self.snapshots),
        ])
    }
}

pub trait TenantConfigRetentionPersistencePort: Send + Sync {
    fn preview(&self, now: DateTime<Utc>) -> PersistenceFuture<'_, TenantConfigArtifactCounts>;

    fn cleanup_packages(
        &self,
        before: DateTime<Utc>,
        batch_size: usize,
        maximum: usize,
    ) -> PersistenceFuture<'_, RetentionCleanupResult>;

    fn cleanup_snapshots(
        &self,
        before: DateTime<Utc>,
        batch_size: usize,
        maximum: usize,
    ) -> PersistenceFuture<'_, RetentionCleanupResult>;
}
