use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};

use crate::PersistenceTransaction;

#[derive(Debug)]
pub struct MenuRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub menu_type: String,
    pub perm_id: Option<i64>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct MenuTreeRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub menu_type: String,
    pub perm_id: Option<i64>,
    pub perm_code: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub children: Vec<MenuTreeRecord>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MenuFilter<'a> {
    pub name: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[async_trait]
pub trait MenuReadPort: Send + Sync {
    async fn find_tree(&self, tenant_id: &str) -> AppResult<Vec<MenuTreeRecord>>;

    async fn find_tree_by_permissions(
        &self,
        tenant_id: &str,
        permission_codes: &[String],
    ) -> AppResult<Vec<MenuTreeRecord>>;

    async fn find_session_tree(
        &self,
        tenant_id: &str,
        permission_codes: &[String],
        excluded_routes: &[String],
    ) -> AppResult<Vec<MenuTreeRecord>>;

    async fn find_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: MenuFilter<'_>,
    ) -> AppResult<PageResult<MenuRecord>>;

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<MenuRecord>>;
}

#[async_trait]
pub trait MenuWriteTransaction: PersistenceTransaction + Sync {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<MenuRecord>>;

    async fn permission_exists_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool>;

    async fn find_by_route_key_for_update(
        &self,
        tenant_id: &str,
        route_key: &str,
    ) -> AppResult<Option<MenuRecord>>;

    async fn insert(&self, tenant_id: &str, record: MenuRecord) -> AppResult<MenuRecord>;

    async fn update(&self, tenant_id: &str, record: MenuRecord) -> AppResult<MenuRecord>;

    async fn has_child_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool>;

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn increment_authorization_epoch(&self, tenant_id: &str) -> AppResult<i32>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait MenuWritePort: Send + Sync {
    async fn begin(&self) -> AppResult<Box<dyn MenuWriteTransaction>>;
}
