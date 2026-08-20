use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::{ControlTransaction, PersistenceFuture};

#[derive(Debug)]
pub struct PermissionRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub parent_id: Option<i64>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub trait PermissionReadPort: Send + Sync {
    fn find_role_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<String>>;

    fn find_role_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>>;

    fn find_all<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Vec<PermissionRecord>>;

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<PermissionRecord>>;
}

pub trait PermissionWriteTransaction: ControlTransaction + Sync {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<PermissionRecord>>;

    fn find_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<PermissionRecord>>;

    fn find_all_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<PermissionRecord>>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PermissionRecord,
    ) -> PersistenceFuture<'a, PermissionRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PermissionRecord,
    ) -> PersistenceFuture<'a, PermissionRecord>;

    fn is_referenced<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, bool>;

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;

    fn filter_syncable_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        codes: BTreeSet<String>,
    ) -> PersistenceFuture<'a, BTreeSet<String>>;

    fn increment_authorization_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i32>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait PermissionWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn PermissionWriteTransaction>>;
}
