use async_trait::async_trait;
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

pub type TenantDataRow = Vec<Option<String>>;

#[derive(Debug, Eq, PartialEq)]
pub struct TenantDataRowBatch {
    pub rows: Vec<TenantDataRow>,
    pub next_cursor: Option<Vec<String>>,
}

/// 租户数据迁移的目标 fence、清理与 catalog 生命周期端口。
#[async_trait]
pub trait TenantDataMigrationPort: Send + Sync {
    fn catalog_tables(&self) -> Vec<TenantDataCatalogTable>;

    async fn prepare_target(&self, fence: TenantDataFence<'_>) -> AppResult<()>;
    async fn clear_prepared_target(&self, fence: TenantDataFence<'_>) -> AppResult<()>;
    async fn freeze_fence(&self, fence: TenantDataFence<'_>) -> AppResult<()>;
    async fn activate_fence(&self, fence: TenantDataFence<'_>) -> AppResult<()>;
    async fn assert_frozen_fence(&self, fence: TenantDataFence<'_>) -> AppResult<()>;
    async fn cleanup_ownership(
        &self,
        fence: TenantDataFence<'_>,
    ) -> AppResult<TenantDataCleanupOwnership>;
    async fn delete_rows_batch(
        &self,
        fence: TenantDataFence<'_>,
        table: &str,
        batch_size: u32,
    ) -> AppResult<u64>;
    async fn finish_cleanup(&self, fence: TenantDataFence<'_>) -> AppResult<()>;

    async fn read_rows_batch(
        &self,
        target_key: &str,
        tenant_id: &str,
        table: &str,
        cursor: Option<&[String]>,
        batch_size: u32,
    ) -> AppResult<TenantDataRowBatch>;

    async fn write_rows_batch(
        &self,
        fence: TenantDataFence<'_>,
        table: &str,
        rows: &[TenantDataRow],
    ) -> AppResult<()>;

    async fn verify_foreign_keys(
        &self,
        target_key: &str,
        tenant_id: &str,
        table: &str,
    ) -> AppResult<()>;
}
