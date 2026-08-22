use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;

use crate::PersistenceTransaction;

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

#[async_trait]
pub trait PermissionReadPort: Send + Sync {
    async fn find_role_codes(&self, tenant_id: &str, role_ids: &[i64]) -> AppResult<Vec<String>>;

    async fn find_role_ids(&self, tenant_id: &str, role_id: i64) -> AppResult<Vec<i64>>;

    async fn find_all(&self, tenant_id: &str) -> AppResult<Vec<PermissionRecord>>;

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<PermissionRecord>>;
}

#[async_trait]
pub trait PermissionWriteTransaction: PersistenceTransaction + Sync {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<PermissionRecord>>;

    async fn find_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<PermissionRecord>>;

    async fn find_all_for_update(&self, tenant_id: &str) -> AppResult<Vec<PermissionRecord>>;

    async fn insert(
        &self,
        tenant_id: &str,
        record: PermissionRecord,
    ) -> AppResult<PermissionRecord>;

    async fn update(
        &self,
        tenant_id: &str,
        record: PermissionRecord,
    ) -> AppResult<PermissionRecord>;

    async fn is_referenced(&self, tenant_id: &str, id: i64) -> AppResult<bool>;

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn filter_syncable_codes(
        &self,
        tenant_id: &str,
        codes: BTreeSet<String>,
    ) -> AppResult<BTreeSet<String>>;

    async fn increment_authorization_epoch(&self, tenant_id: &str) -> AppResult<i32>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait PermissionWritePort: Send + Sync {
    async fn begin(&self) -> AppResult<Box<dyn PermissionWriteTransaction>>;
}
