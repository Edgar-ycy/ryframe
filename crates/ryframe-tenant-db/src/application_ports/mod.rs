mod migration;
mod provisioning;
mod runtime;
mod targets;

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
        TenantDataError::UnknownTarget { target_key }
        | TenantDataError::TargetUnavailable { target_key } => {
            AppError::TenantDataTargetUnavailable(format!("租户数据目标不可用: {target_key}"), 5)
        }
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
