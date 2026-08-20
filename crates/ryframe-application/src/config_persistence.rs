use chrono::{DateTime, Utc};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

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

pub trait ConfigTransaction: ControlTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_by_key_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        key: &'a str,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>>;

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: ConfigRecord,
    ) -> PersistenceFuture<'a, ConfigRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: ConfigRecord,
    ) -> PersistenceFuture<'a, ConfigRecord>;

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;

    fn record_namespace_change<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> PersistenceFuture<'a, i64>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()>;
}

pub trait ConfigPersistencePort: Send + Sync {
    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: ConfigFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<ConfigRecord>>;

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: ConfigFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<ConfigRecord>>;

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>>;

    fn find_by_key<'a>(
        &'a self,
        tenant_id: &'a str,
        key: &'a str,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>>;

    fn find_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> PersistenceFuture<'a, i64>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ConfigTransaction>>;
}
