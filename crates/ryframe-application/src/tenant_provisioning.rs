use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;
use sea_orm::DatabaseTransaction;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantProvisioningPlacement {
    pub tenant_id: String,
    pub target_key: String,
    pub generation: i64,
    pub switch_token: String,
}

pub type TenantProvisioningFuture<'a> = Pin<Box<dyn Future<Output = AppResult<()>> + Send + 'a>>;

/// 租户创建 Saga 所需的数据放置与 fence 端口。
pub trait TenantProvisioningPort: Send + Sync {
    fn prepare(
        &self,
        tenant_id: String,
        target_key: String,
        generation: i64,
        switch_token: String,
    ) -> AppResult<TenantProvisioningPlacement>;

    fn create_pending<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a>;

    fn create_or_resume_pending<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a>;

    fn provision_fence<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a>;

    fn activate<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a>;

    fn fail<'a>(
        &'a self,
        transaction: &'a DatabaseTransaction,
        placement: &'a TenantProvisioningPlacement,
    ) -> TenantProvisioningFuture<'a>;
}
