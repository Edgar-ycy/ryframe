use ryframe_application::TenantBusinessDataState as ApplicationTenantBusinessDataState;
use ryframe_kernel::AppError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

macro_rules! fixed_string_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                #[serde(rename = $value)]
                #[schema(rename = $value)]
                $variant,
            )+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = AppError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => {
                        tracing::error!(
                            enum_name = stringify!($name),
                            wire_value = value,
                            "服务层返回了未识别的固定值"
                        );
                        Err(AppError::Internal(format!(
                            "{} 包含未识别的固定值",
                            stringify!($name)
                        )))
                    }
                }
            }
        }
    };
}

fixed_string_enum! {
    /// 产品套餐是否允许被使用。Wire value 沿用控制库既有的 `0`/`1`。
    pub enum ProductPlanStatus {
        Disabled => "0",
        Enabled => "1",
    }
}

fixed_string_enum! {
    /// 产品套餐版本生命周期。
    pub enum ProductPlanVersionStatus {
        Draft => "draft",
        Published => "published",
        Retired => "retired",
    }
}

fixed_string_enum! {
    /// 有效能力配置的来源。
    pub enum EffectiveCapabilitySource {
        Plan => "plan",
        Override => "override",
        NotEntitled => "none",
    }
}

fixed_string_enum! {
    /// 租户业务数据控制面状态。
    pub enum TenantBusinessDataState {
        Provisioning => "provisioning",
        Active => "active",
        Maintenance => "maintenance",
        Failed => "failed",
    }
}

fixed_string_enum! {
    /// 数据目标占用模式。
    pub enum DataTargetMode {
        Shared => "shared",
        Dedicated => "dedicated",
    }
}

fixed_string_enum! {
    /// 数据目标连接来源。
    pub enum DataTargetKind {
        Control => "control",
        Mysql => "mysql",
    }
}

fixed_string_enum! {
    /// 数据目标健康快照。
    pub enum DataTargetHealth {
        Unknown => "unknown",
        Verified => "verified",
        Unavailable => "unavailable",
    }
}

fixed_string_enum! {
    /// 数据目标资格检查用途。
    pub enum DataTargetEligibility {
        NewTenant => "new_tenant",
        Migration => "migration",
    }
}

fixed_string_enum! {
    /// 数据库平台恢复点的隔离粒度。
    pub enum BackupPointScope {
        Tenant => "tenant",
        Shard => "shard",
    }
}

fixed_string_enum! {
    /// 数据库平台恢复点校验状态。
    pub enum BackupPointValidationStatus {
        Pending => "pending",
        Valid => "valid",
        Invalid => "invalid",
    }
}

fixed_string_enum! {
    /// 停写迁移状态机。
    pub enum TenantDataMigrationState {
        Prechecking => "prechecking",
        Queued => "queued",
        Quiescing => "quiescing",
        Frozen => "frozen",
        Copying => "copying",
        Verifying => "verifying",
        CuttingOver => "cutting_over",
        Activating => "activating",
        Succeeded => "succeeded",
        RetentionPending => "retention_pending",
        Finalized => "finalized",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

fixed_string_enum! {
    /// 单表复制与校验检查点状态。
    pub enum TenantDataMigrationItemState {
        Pending => "pending",
        Copying => "copying",
        Copied => "copied",
        Verifying => "verifying",
        Verified => "verified",
        Failed => "failed",
    }
}

fixed_string_enum! {
    /// 单表源数据清理检查点状态。
    pub enum TenantDataMigrationCleanupState {
        Pending => "pending",
        Cleaning => "cleaning",
        Cleaned => "cleaned",
    }
}

impl From<ApplicationTenantBusinessDataState> for TenantBusinessDataState {
    fn from(value: ApplicationTenantBusinessDataState) -> Self {
        match value {
            ApplicationTenantBusinessDataState::Provisioning => Self::Provisioning,
            ApplicationTenantBusinessDataState::Active => Self::Active,
            ApplicationTenantBusinessDataState::Maintenance => Self::Maintenance,
            ApplicationTenantBusinessDataState::Failed => Self::Failed,
        }
    }
}
