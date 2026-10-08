use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::PersistenceTransaction;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigRecord {
    pub id: i64,
    pub name: String,
    pub key: String,
    pub value: String,
    pub portable: bool,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigFilter<'a> {
    pub name: Option<&'a str>,
    pub key: Option<&'a str>,
}

#[async_trait]
pub trait ConfigTransaction: PersistenceTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_by_key_for_update(
        &self,
        tenant_id: &str,
        key: &str,
    ) -> AppResult<Option<ConfigRecord>>;

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<ConfigRecord>>;

    async fn insert(&self, tenant_id: &str, record: ConfigRecord) -> AppResult<ConfigRecord>;

    async fn update(&self, tenant_id: &str, record: ConfigRecord) -> AppResult<ConfigRecord>;

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn record_namespace_change(&self, tenant_id: &str, namespace: &str) -> AppResult<i64>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait ConfigPersistencePort: Send + Sync {
    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: ConfigFilter<'_>,
    ) -> AppResult<PageResult<ConfigRecord>>;

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: ConfigFilter<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<ConfigRecord>>;

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<ConfigRecord>>;

    async fn find_by_key(&self, tenant_id: &str, key: &str) -> AppResult<Option<ConfigRecord>>;

    async fn find_namespace_version(&self, tenant_id: &str, namespace: &str) -> AppResult<i64>;

    async fn begin(&self) -> AppResult<Box<dyn ConfigTransaction>>;
}
