use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

/// 应用层可识别的租户业务数据状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantBusinessDataState {
    Provisioning,
    Active,
    Maintenance,
    Failed,
}

impl TenantBusinessDataState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Active => "active",
            Self::Maintenance => "maintenance",
            Self::Failed => "failed",
        }
    }
}

/// 认证、路由和实时通知共用的强一致租户运行时快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantRuntimeSnapshot {
    tenant_id: String,
    authorization_epoch: u64,
    runtime_epoch: u64,
    placement_generation: i64,
    business_data_state: TenantBusinessDataState,
}

impl TenantRuntimeSnapshot {
    pub fn new(
        tenant_id: String,
        authorization_epoch: u64,
        runtime_epoch: u64,
        placement_generation: i64,
        business_data_state: TenantBusinessDataState,
    ) -> Self {
        Self {
            tenant_id,
            authorization_epoch,
            runtime_epoch,
            placement_generation,
            business_data_state,
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub const fn authorization_epoch(&self) -> u64 {
        self.authorization_epoch
    }

    pub const fn runtime_epoch(&self) -> u64 {
        self.runtime_epoch
    }

    pub const fn placement_generation(&self) -> i64 {
        self.placement_generation
    }

    pub const fn business_data_state(&self) -> TenantBusinessDataState {
        self.business_data_state
    }
}

pub type TenantRuntimeReadFuture<'a> =
    Pin<Box<dyn Future<Output = AppResult<TenantRuntimeSnapshot>> + Send + 'a>>;

/// 租户运行时快照读取端口，具体控制库路由由组合根注入。
pub trait TenantRuntimeReadPort: Send + Sync {
    fn runtime_snapshot<'a>(&'a self, tenant_id: &'a str) -> TenantRuntimeReadFuture<'a>;
}
