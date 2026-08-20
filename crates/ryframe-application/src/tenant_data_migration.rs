use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

#[derive(Clone, Copy, Debug)]
pub struct TenantDataFence<'a> {
    pub tenant_id: &'a str,
    pub target_key: &'a str,
    pub generation: i64,
    pub switch_token: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDataCleanupOwnership {
    OwnedFrozen,
    AlreadyClean,
    NotOwned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantDataCatalogTable {
    pub name: &'static str,
    pub copy_order: u32,
}

pub type TenantDataMigrationFuture<'a, T = ()> =
    Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// 租户数据迁移的目标 fence、清理与 catalog 生命周期端口。
pub trait TenantDataMigrationPort: Send + Sync {
    fn catalog_tables(&self) -> Vec<TenantDataCatalogTable>;

    fn prepare_target<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a>;
    fn clear_prepared_target<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a>;
    fn freeze_fence<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a>;
    fn activate_fence<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a>;
    fn assert_frozen_fence<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a>;
    fn cleanup_ownership<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a, TenantDataCleanupOwnership>;
    fn delete_rows_batch<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
        table: &'a str,
        batch_size: u32,
    ) -> TenantDataMigrationFuture<'a, u64>;
    fn finish_cleanup<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a>;
}
