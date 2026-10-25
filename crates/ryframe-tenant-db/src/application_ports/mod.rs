pub mod backup;
pub mod data_scope_projection;
pub mod tenant_data;
pub mod tenants;

use ryframe_kernel::AppError;

use crate::TenantDataError;

fn map_error(error: TenantDataError) -> AppError {
    match error {
        TenantDataError::InvalidConfiguration(message) => AppError::Config(message),
        TenantDataError::InvalidTenantId(message) => AppError::Validation(message),
        TenantDataError::StalePlacementGeneration {
            tenant_id,
            session_generation,
            current_generation,
        } => AppError::StalePlacementGeneration(format!(
            "tenant={tenant_id}, session={session_generation}, current={current_generation}"
        )),
        TenantDataError::TenantDataMaintenance {
            tenant_id,
            generation,
        } => AppError::TenantDataMaintenance(
            format!("tenant={tenant_id}, generation={generation}"),
            5,
        ),
        TenantDataError::FenceRejected {
            tenant_id,
            target_key,
            ..
        } => AppError::TenantDataMaintenance(format!("tenant={tenant_id}, target={target_key}"), 5),
        TenantDataError::DedicatedTargetOccupied { target_key } => {
            AppError::TenantOperationConflict(format!("专属租户数据目标已被占用: {target_key}"))
        }
        TenantDataError::DataScopeProjectionStale {
            tenant_id,
            expected,
            actual,
        } => AppError::TenantDataTargetUnavailable(
            format!(
                "租户 {tenant_id} 的数据范围投影暂不可用: expected={expected}, actual={actual:?}；请等待投影同步后重试"
            ),
            5,
        ),
        TenantDataError::UnknownTarget { target_key }
        | TenantDataError::TargetUnavailable {
            target_key,
            source: _,
        } => AppError::TenantDataTargetUnavailable(format!("租户数据目标不可用: {target_key}"), 5),
        TenantDataError::PlacementUnavailable { tenant_id }
        | TenantDataError::InvalidPlacement { tenant_id, .. } => {
            AppError::TenantDataTargetUnavailable(format!("租户数据放置不可用: {tenant_id}"), 5)
        }
        TenantDataError::PoolCapacityExhausted { .. }
        | TenantDataError::ConnectionBudgetExhausted { .. } => {
            AppError::TenantDataTargetUnavailable("租户数据连接池容量不足".into(), 5)
        }
    }
}

#[cfg(test)]
mod cause_tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn target_unavailable_discards_internal_cause_at_application_boundary() {
        let config = ryframe_config::DbConnection::default();
        let source =
            crate::SafeDatabaseCause::from_connection(&config, "internal-driver-cause-1045");
        let mapped = map_error(TenantDataError::TargetUnavailable {
            target_key: "private-target".into(),
            source: Some(source),
        });
        assert!(matches!(
            &mapped,
            AppError::TenantDataTargetUnavailable(_, 5)
        ));
        assert!(!format!("{mapped:?}").contains("internal-driver-cause"));
        assert!(mapped.source().is_none());
    }
}
