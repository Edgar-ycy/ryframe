use async_trait::async_trait;
use ryframe_kernel::AppResult;

use crate::PersistenceTransaction;

use super::RoleRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolePermissionRef {
    pub id: i64,
    pub code: String,
}

#[async_trait]
pub trait RoleWriteTransaction: PersistenceTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<RoleRecord>>;

    async fn find_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<RoleRecord>>;

    async fn count_available_super_roles(&self, tenant_id: &str) -> AppResult<usize>;

    async fn ensure_role_quota(&self, tenant_id: &str) -> AppResult<()>;

    async fn insert(&self, tenant_id: &str, record: RoleRecord) -> AppResult<RoleRecord>;

    async fn update(&self, tenant_id: &str, record: RoleRecord) -> AppResult<RoleRecord>;

    async fn delete_many(&self, tenant_id: &str, ids: &[i64]) -> AppResult<u64>;

    async fn find_permissions_for_update(
        &self,
        tenant_id: &str,
        permission_ids: &[i64],
    ) -> AppResult<Vec<RolePermissionRef>>;

    async fn ensure_permission_codes_enabled(
        &self,
        tenant_id: &str,
        permission_codes: &[String],
    ) -> AppResult<()>;

    async fn assign_permissions(
        &self,
        tenant_id: &str,
        role_id: i64,
        permission_ids: &[i64],
    ) -> AppResult<()>;

    async fn find_departments_for_update(
        &self,
        tenant_id: &str,
        department_ids: &[i64],
    ) -> AppResult<Vec<i64>>;

    async fn replace_data_scope(
        &self,
        tenant_id: &str,
        role_id: i64,
        data_scope: &str,
        department_ids: &[i64],
    ) -> AppResult<()>;

    async fn increment_authorization_epoch(&self, tenant_id: &str) -> AppResult<i32>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait RoleWritePort: Send + Sync {
    async fn begin(&self) -> AppResult<Box<dyn RoleWriteTransaction>>;
}
