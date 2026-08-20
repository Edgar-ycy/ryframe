use chrono::{DateTime, Utc};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

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

pub trait DictTransaction: ControlTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_type_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<DictTypeRecord>>;

    fn find_type_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<DictTypeRecord>>;

    fn insert_type<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictTypeRecord,
    ) -> PersistenceFuture<'a, DictTypeRecord>;

    fn update_type<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictTypeRecord,
    ) -> PersistenceFuture<'a, DictTypeRecord>;

    fn delete_type<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;

    fn find_data_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<DictDataRecord>>;

    fn insert_data<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictDataRecord,
    ) -> PersistenceFuture<'a, DictDataRecord>;

    fn update_data<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictDataRecord,
    ) -> PersistenceFuture<'a, DictDataRecord>;

    fn delete_data<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()>;
}

pub trait DictPersistencePort: Send + Sync {
    fn find_types_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: DictTypeFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<DictTypeRecord>>;

    fn find_type_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: DictTypeFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<DictTypeRecord>>;

    fn find_data_by_type<'a>(
        &'a self,
        tenant_id: &'a str,
        type_code: &'a str,
    ) -> PersistenceFuture<'a, Vec<DictDataRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn DictTransaction>>;
}
