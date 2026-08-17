use std::fmt;

use serde::{Deserialize, Serialize};
use validator::ValidationErrors;

/// 可长期稳定使用的领域错误码。
///
/// 序列化键是当前唯一的跨进程契约；已有键不得改名或复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Validation,
    Authentication,
    Authorization,
    NotFound,
    Conflict,
    PayloadTooLarge,
    RateLimited,
    Database,
    Config,
    Internal,
    ServiceUnavailable,
    CapabilityUnavailable,
    TenantCapabilityDenied,
    PermissionDenied,
    StaleRuntimeEpoch,
    StalePlacementGeneration,
    TenantOperationConflict,
    TenantDataMaintenance,
    TenantDataTargetUnavailable,
}

impl ErrorCode {
    /// 返回稳定的机器可读错误键。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::RateLimited => "rate_limited",
            Self::Database => "database",
            Self::Config => "config",
            Self::Internal => "internal",
            Self::ServiceUnavailable => "service_unavailable",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::TenantCapabilityDenied => "tenant_capability_denied",
            Self::PermissionDenied => "permission_denied",
            Self::StaleRuntimeEpoch => "stale_runtime_epoch",
            Self::StalePlacementGeneration => "stale_placement_generation",
            Self::TenantOperationConflict => "tenant_operation_conflict",
            Self::TenantDataMaintenance => "tenant_data_maintenance",
            Self::TenantDataTargetUnavailable => "tenant_data_target_unavailable",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 与传输协议无关的应用错误。
///
/// HTTP 状态码、响应信封与日志脱敏应由边缘适配层处理，领域层仅表达失败语义及细节。
#[derive(Debug)]
pub enum AppError {
    Validation(String),
    Authentication(String),
    Authorization(String),
    NotFound(String),
    Conflict(String),
    /// 资源当前被短期占用；调用方可在指定秒数后安全重试。
    RetryableConflict(String, u64),
    PayloadTooLarge(String),
    RateLimited(String, u64),
    Database(String),
    Config(String),
    Internal(String),
    ServiceUnavailable(String),
    /// 当前部署缺少 Capability 声明的基础设施依赖。
    CapabilityUnavailable(String),
    /// 当前租户未被套餐或覆盖授予所请求的 Capability。
    TenantCapabilityDenied(String),
    /// 当前主体缺少 RBAC 权限。
    PermissionDenied(String),
    /// 产品上下文写入基于过期的租户运行纪元。
    StaleRuntimeEpoch(String),
    /// 数据放置或迁移写入基于过期的放置代次。
    StalePlacementGeneration(String),
    /// 同一租户已有互斥的控制面操作。
    TenantOperationConflict(String),
    /// 租户业务数据正在维护窗口内停写。
    TenantDataMaintenance(String, u64),
    /// 租户绑定的数据目标当前不可用。
    TenantDataTargetUnavailable(String, u64),
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = match self {
            Self::Validation(_) => "参数校验失败",
            Self::Authentication(_) => "认证失败",
            Self::Authorization(_) => "权限不足",
            Self::NotFound(_) => "资源不存在",
            Self::Conflict(_) => "数据冲突",
            Self::RetryableConflict(_, _) => "资源暂时被占用",
            Self::PayloadTooLarge(_) => "请求体过大",
            Self::RateLimited(_, _) => "请求过于频繁",
            Self::Database(_) => "数据库错误",
            Self::Config(_) => "配置错误",
            Self::Internal(_) => "内部错误",
            Self::ServiceUnavailable(_) => "服务暂不可用",
            Self::CapabilityUnavailable(_) => "部署能力不可用",
            Self::TenantCapabilityDenied(_) => "租户未开通能力",
            Self::PermissionDenied(_) => "权限不足",
            Self::StaleRuntimeEpoch(_) => "租户运行版本已变化",
            Self::StalePlacementGeneration(_) => "租户数据放置已变化",
            Self::TenantOperationConflict(_) => "租户操作冲突",
            Self::TenantDataMaintenance(_, _) => "租户业务数据维护中",
            Self::TenantDataTargetUnavailable(_, _) => "租户业务数据库不可用",
        };
        write!(formatter, "{category}: {}", self.message())
    }
}

impl std::error::Error for AppError {}

impl AppError {
    /// 返回错误对应的稳定领域错误码。
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) => ErrorCode::Validation,
            Self::Authentication(_) => ErrorCode::Authentication,
            Self::Authorization(_) => ErrorCode::Authorization,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::RetryableConflict(_, _) => ErrorCode::Conflict,
            Self::PayloadTooLarge(_) => ErrorCode::PayloadTooLarge,
            Self::RateLimited(_, _) => ErrorCode::RateLimited,
            Self::Database(_) => ErrorCode::Database,
            Self::Config(_) => ErrorCode::Config,
            Self::Internal(_) => ErrorCode::Internal,
            Self::ServiceUnavailable(_) => ErrorCode::ServiceUnavailable,
            Self::CapabilityUnavailable(_) => ErrorCode::CapabilityUnavailable,
            Self::TenantCapabilityDenied(_) => ErrorCode::TenantCapabilityDenied,
            Self::PermissionDenied(_) => ErrorCode::PermissionDenied,
            Self::StaleRuntimeEpoch(_) => ErrorCode::StaleRuntimeEpoch,
            Self::StalePlacementGeneration(_) => ErrorCode::StalePlacementGeneration,
            Self::TenantOperationConflict(_) => ErrorCode::TenantOperationConflict,
            Self::TenantDataMaintenance(_, _) => ErrorCode::TenantDataMaintenance,
            Self::TenantDataTargetUnavailable(_, _) => ErrorCode::TenantDataTargetUnavailable,
        }
    }

    /// 返回适合领域层记录或进一步适配的原始错误消息。
    pub fn message(&self) -> &str {
        match self {
            Self::Validation(message)
            | Self::Authentication(message)
            | Self::Authorization(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::RetryableConflict(message, _)
            | Self::PayloadTooLarge(message)
            | Self::Database(message)
            | Self::Config(message)
            | Self::Internal(message)
            | Self::ServiceUnavailable(message) => message,
            Self::CapabilityUnavailable(message)
            | Self::TenantCapabilityDenied(message)
            | Self::PermissionDenied(message)
            | Self::StaleRuntimeEpoch(message)
            | Self::StalePlacementGeneration(message)
            | Self::TenantOperationConflict(message)
            | Self::TenantDataMaintenance(message, _)
            | Self::TenantDataTargetUnavailable(message, _) => message,
            Self::RateLimited(message, _) => message,
        }
    }

    /// 返回限流或短期资源占用错误的建议重试等待秒数，其他错误返回 `None`。
    pub const fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited(_, retry_after_secs)
            | Self::RetryableConflict(_, retry_after_secs)
            | Self::TenantDataMaintenance(_, retry_after_secs)
            | Self::TenantDataTargetUnavailable(_, retry_after_secs) => Some(*retry_after_secs),
            _ => None,
        }
    }
}

impl From<ValidationErrors> for AppError {
    fn from(error: ValidationErrors) -> Self {
        Self::Validation(error.to_string())
    }
}
