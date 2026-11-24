use async_trait::async_trait;
use ryframe_application::ports::tenant_data::{
    TenantDataPoolStats, TenantDataTargetAccess, TenantDataTargetHealth, TenantDataTargetMetadata,
    TenantDataTargetPort,
};
use ryframe_kernel::{AppError, AppResult};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;

use crate::{
    TenantDatabaseRouter, TenantDatabaseTargetHealthStatus, migration::tenant_data_catalog,
};

use super::super::map_error;

#[async_trait]
impl TenantDataTargetPort for TenantDatabaseRouter {
    fn contains(&self, target_key: &str) -> bool {
        self.targets().contains(target_key)
    }

    fn is_dedicated(&self, target_key: &str) -> Option<bool> {
        self.targets().target_is_dedicated(target_key)
    }

    fn mode_code(&self, target_key: &str) -> Option<&'static str> {
        self.targets().target_mode_code(target_key)
    }

    fn kind_code(&self, target_key: &str) -> Option<&'static str> {
        self.targets().target_kind_code(target_key)
    }

    fn catalog_fingerprint(&self) -> String {
        tenant_data_catalog().schema_fingerprint()
    }

    fn catalog_table_count(&self) -> usize {
        tenant_data_catalog().tables().len()
    }

    async fn metadata(&self) -> AppResult<Vec<TenantDataTargetMetadata>> {
        self.targets()
            .metadata()
            .await
            .into_iter()
            .map(|metadata| {
                let mode = metadata.mode_code().into();
                let kind = metadata.kind_code().into();
                let last_verified_at = metadata.last_verified_at.map(utc_time).transpose()?;
                Ok(TenantDataTargetMetadata {
                    key: metadata.key,
                    display_name: metadata.display_name,
                    region: metadata.region,
                    mode,
                    kind,
                    connected: metadata.connected,
                    pool_max_connections: metadata.pool_max_connections,
                    active_leases: metadata.active_leases,
                    schema_fingerprint: metadata.schema_fingerprint,
                    health: map_health(metadata.health),
                    last_verified_at,
                })
            })
            .collect()
    }

    async fn pool_stats(&self) -> AppResult<TenantDataPoolStats> {
        let stats = self.targets().pool_stats().await;
        Ok(TenantDataPoolStats {
            reserved_connections: stats.reserved_connections,
            max_total_connections: stats.max_total_connections,
            open_targets: stats.open_targets,
            opening_targets: stats.opening_targets,
        })
    }

    async fn verify_now(&self, target_key: &str) -> AppResult<()> {
        self.verify_target_now_for_catalog(target_key, tenant_data_catalog())
            .await
            .map_err(map_error)
    }

    async fn validate_catalog(&self, target_key: &str) -> AppResult<TenantDataTargetAccess> {
        self.open_target_for_catalog(target_key, tenant_data_catalog())
            .await
            .map(|target| TenantDataTargetAccess {
                dedicated: target.is_dedicated(),
            })
            .map_err(map_error)
    }

    async fn is_occupied(&self, target_key: &str) -> AppResult<bool> {
        self.target_occupancy_for_catalog(target_key, tenant_data_catalog())
            .await
            .map(|occupancy| occupancy.is_some())
            .map_err(map_error)
    }

    async fn tenant_is_empty(&self, target_key: &str, tenant_id: &str) -> AppResult<bool> {
        self.tenant_is_empty_on_target_for_catalog(target_key, tenant_id, tenant_data_catalog())
            .await
            .map_err(map_error)
    }
}

pub const fn map_health(health: TenantDatabaseTargetHealthStatus) -> TenantDataTargetHealth {
    match health {
        TenantDatabaseTargetHealthStatus::Unknown => TenantDataTargetHealth::Unknown,
        TenantDatabaseTargetHealthStatus::Verified => TenantDataTargetHealth::Verified,
        TenantDatabaseTargetHealthStatus::Unavailable => TenantDataTargetHealth::Unavailable,
    }
}

fn utc_time(value: SystemTime) -> AppResult<OffsetDateTime> {
    let (duration, sign) = match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => (duration, 1_i128),
        Err(error) => (error.duration(), -1_i128),
    };
    // u64 秒乘十亿仍在 i128 范围内；整体取负保留纪元之前的子秒。
    let nanos = sign
        * (i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos()));
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|_| AppError::Internal("数据目标核验时间超出 UTC 表示范围".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn verified_at_preserves_signed_nanoseconds_and_rejects_out_of_range() {
        for nanos in [
            0,
            100,
            1_000,
            120_000_000,
            123_400_000,
            123_456_700,
            999_999_900,
            1_123_456_700,
            12_123_456_700,
        ] {
            let duration = Duration::from_nanos(nanos);
            for (value, expected) in [
                (UNIX_EPOCH.checked_add(duration).unwrap(), i128::from(nanos)),
                (
                    UNIX_EPOCH.checked_sub(duration).unwrap(),
                    -i128::from(nanos),
                ),
            ] {
                let utc = utc_time(value).unwrap();
                assert_eq!(utc.offset(), time::UtcOffset::UTC);
                assert_eq!(utc.unix_timestamp_nanos(), expected);
            }
        }
        let out_of_range = UNIX_EPOCH
            .checked_add(Duration::from_secs(253_402_300_800))
            .unwrap();
        assert!(matches!(utc_time(out_of_range), Err(AppError::Internal(_))));
    }
}
