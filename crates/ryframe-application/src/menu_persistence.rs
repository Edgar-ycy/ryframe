use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

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

pub trait MenuReadPort: Send + Sync {
    fn find_tree<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Vec<MenuTreeRecord>>;

    fn find_tree_by_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> PersistenceFuture<'a, Vec<MenuTreeRecord>>;

    fn find_session_tree<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
        excluded_routes: &'a [String],
    ) -> PersistenceFuture<'a, Vec<MenuTreeRecord>>;

    fn find_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: MenuFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<MenuRecord>>;

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<MenuRecord>>;
}

pub trait MenuWriteTransaction: ControlTransaction + Sync {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<MenuRecord>>;

    fn permission_exists_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn find_by_route_key_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        route_key: &'a str,
    ) -> PersistenceFuture<'a, Option<MenuRecord>>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: MenuRecord,
    ) -> PersistenceFuture<'a, MenuRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: MenuRecord,
    ) -> PersistenceFuture<'a, MenuRecord>;

    fn has_child_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;

    fn increment_authorization_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i32>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()>;
}

pub trait MenuWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn MenuWriteTransaction>>;
}
