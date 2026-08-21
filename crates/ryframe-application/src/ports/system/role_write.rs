use crate::{ControlTransaction, PersistenceFuture};

use super::RoleRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolePermissionRef {
    pub id: i64,
    pub code: String,
}

pub trait RoleWriteTransaction: ControlTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<RoleRecord>>;

    fn find_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<RoleRecord>>;

    fn count_available_super_roles<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, usize>;

    fn ensure_role_quota<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: RoleRecord,
    ) -> PersistenceFuture<'a, RoleRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: RoleRecord,
    ) -> PersistenceFuture<'a, RoleRecord>;

    fn delete_many<'a>(&'a self, tenant_id: &'a str, ids: &'a [i64]) -> PersistenceFuture<'a, u64>;

    fn find_permissions_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<RolePermissionRef>>;

    fn ensure_permission_codes_enabled<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> PersistenceFuture<'a, ()>;

    fn assign_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
        permission_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()>;

    fn find_departments_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        department_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<i64>>;

    fn replace_data_scope<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
        data_scope: &'a str,
        department_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()>;

    fn increment_authorization_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i32>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()>;
}

pub trait RoleWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn RoleWriteTransaction>>;
}
