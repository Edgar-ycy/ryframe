use super::*;

pub(super) fn classify_error(error: &AppError) -> (i32, &'static str, &'static str) {
    match error {
        AppError::Authentication(_) => (401, "invalid_credential", RESULT_DENIED),
        AppError::Authorization(_) => (403, "capability_denied", RESULT_DENIED),
        AppError::NotFound(_) => (404, "not_found", RESULT_DENIED),
        AppError::Validation(_) => (400, "validation", RESULT_DENIED),
        AppError::PayloadTooLarge(_) => (413, "response_too_large", RESULT_ERROR),
        AppError::RateLimited(_, _) => (429, "rate_limited", RESULT_DENIED),
        AppError::Conflict(_) | AppError::RetryableConflict(_, _) => {
            (409, "conflict", RESULT_DENIED)
        }
        AppError::Database(_) => (503, "database_unavailable", RESULT_ERROR),
        AppError::ServiceUnavailable(message) if message == "Agent 查询超时" => {
            (503, "query_timeout", RESULT_ERROR)
        }
        AppError::ServiceUnavailable(_) => (503, "service_unavailable", RESULT_ERROR),
        AppError::CapabilityUnavailable(_) => (501, "capability_unavailable", RESULT_ERROR),
        AppError::TenantCapabilityDenied(_) => (403, "tenant_capability_denied", RESULT_DENIED),
        AppError::PermissionDenied(_) => (403, "permission_denied", RESULT_DENIED),
        AppError::StaleRuntimeEpoch(_) => (409, "stale_runtime_epoch", RESULT_DENIED),
        AppError::StalePlacementGeneration(_) => (409, "stale_placement_generation", RESULT_DENIED),
        AppError::TenantOperationConflict(_) => (409, "tenant_operation_conflict", RESULT_DENIED),
        AppError::TenantDataMaintenance(_, _) => (423, "tenant_data_maintenance", RESULT_ERROR),
        AppError::TenantDataTargetUnavailable(_, _) => {
            (503, "tenant_data_target_unavailable", RESULT_ERROR)
        }
        AppError::Config(_) | AppError::Internal(_) => (500, "internal", RESULT_ERROR),
    }
}

pub(super) fn classify_pre_authorization_error(
    error: &AppError,
) -> (i32, &'static str, &'static str) {
    match error {
        AppError::Database(_) => (503, "database_unavailable", RESULT_ERROR),
        AppError::ServiceUnavailable(message) if message == "Agent 查询超时" => {
            (503, "query_timeout", RESULT_ERROR)
        }
        AppError::ServiceUnavailable(_) => (503, "service_unavailable", RESULT_ERROR),
        AppError::Config(_) | AppError::Internal(_) => (500, "internal", RESULT_ERROR),
        _ => (401, "invalid_credential", RESULT_DENIED),
    }
}

pub(super) fn mask_missing_identity(error: AppError) -> AppError {
    match error {
        AppError::NotFound(_) | AppError::Authentication(_) | AppError::Authorization(_) => {
            invalid_credential()
        }
        error => error,
    }
}

pub(super) fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

pub(super) async fn before_deadline<T>(
    deadline: tokio::time::Instant,
    future: impl Future<Output = AppResult<T>>,
) -> AppResult<T> {
    match tokio::time::timeout_at(deadline, future).await {
        Ok(result) => result,
        Err(_) => Err(AppError::ServiceUnavailable("Agent 查询超时".into())),
    }
}

pub(super) fn normalized_request_id(value: &str) -> String {
    uuid::Uuid::parse_str(value)
        .map(|request_id| request_id.to_string())
        .unwrap_or_else(|_| uuid::Uuid::now_v7().to_string())
}

pub(super) fn json_value(value: impl Serialize) -> AppResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|_| AppError::Internal("序列化 Agent 数据失败".into()))
}
