use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::PersistenceTransaction;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictTypeRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DictTypeFilter<'a> {
    pub name: Option<&'a str>,
    pub code: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictDataRecord {
    pub id: i64,
    pub type_code: String,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub status: String,
    pub css_class: Option<String>,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[async_trait]
pub trait DictTransaction: PersistenceTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_type_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<DictTypeRecord>>;

    async fn find_type_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<DictTypeRecord>>;

    async fn insert_type(
        &self,
        tenant_id: &str,
        record: DictTypeRecord,
    ) -> AppResult<DictTypeRecord>;

    async fn update_type(
        &self,
        tenant_id: &str,
        record: DictTypeRecord,
    ) -> AppResult<DictTypeRecord>;

    async fn delete_type(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn find_data_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<DictDataRecord>>;

    async fn insert_data(
        &self,
        tenant_id: &str,
        record: DictDataRecord,
    ) -> AppResult<DictDataRecord>;

    async fn update_data(
        &self,
        tenant_id: &str,
        record: DictDataRecord,
    ) -> AppResult<DictDataRecord>;

    async fn delete_data(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait DictPersistencePort: Send + Sync {
    async fn find_types_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: DictTypeFilter<'_>,
    ) -> AppResult<PageResult<DictTypeRecord>>;

    async fn find_type_export_batch(
        &self,
        tenant_id: &str,
        filter: DictTypeFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<DictTypeRecord>>;

    async fn find_data_by_type(
        &self,
        tenant_id: &str,
        type_code: &str,
    ) -> AppResult<Vec<DictDataRecord>>;

    async fn begin(&self) -> AppResult<Box<dyn DictTransaction>>;
}
