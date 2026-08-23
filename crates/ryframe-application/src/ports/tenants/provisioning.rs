use async_trait::async_trait;
use ryframe_kernel::AppResult;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantProvisioningPlacement {
    pub tenant_id: String,
    pub target_key: String,
    pub generation: i64,
    pub switch_token: String,
}

/// 租户创建 Saga 所需的数据放置与 fence 端口。
#[async_trait]
pub trait TenantProvisioningPort: Send + Sync {
    fn prepare(
        &self,
        tenant_id: String,
        target_key: String,
        generation: i64,
        switch_token: String,
    ) -> AppResult<TenantProvisioningPlacement>;

    async fn provision_fence(&self, placement: &TenantProvisioningPlacement) -> AppResult<()>;
}
