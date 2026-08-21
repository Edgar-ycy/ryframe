use chrono::{DateTime, Utc};

use crate::PersistenceFuture;

#[derive(Clone, Debug)]
pub struct DiagnosticRoleRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub status: String,
    pub data_scope: String,
    pub is_super: bool,
}

#[derive(Clone, Debug)]
pub struct DiagnosticPermissionRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct DiagnosticMenuRecord {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub route_key: Option<String>,
    pub perm_id: Option<i64>,
    pub menu_type: String,
    pub status: String,
    pub visible: bool,
}

impl DiagnosticMenuRecord {
    pub fn is_button(&self) -> bool {
        self.menu_type == "F"
    }

    pub fn is_dir(&self) -> bool {
        self.menu_type == "M"
    }

    pub fn is_enabled(&self) -> bool {
        self.status == "1"
    }
}

#[derive(Debug)]
pub struct DiagnosticDepartmentRecord {
    pub id: i64,
    pub name: String,
}

pub trait AuthorizationDiagnosticReadPort: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn user_tenant_id(&self, user_id: i64) -> PersistenceFuture<'_, Option<String>>;

    fn assigned_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<DiagnosticRoleRecord>>;

    fn permissions<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<DiagnosticPermissionRecord>>;

    fn role_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Vec<DiagnosticPermissionRecord>>;

    fn menus<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Vec<DiagnosticMenuRecord>>;

    fn accessible_menu_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> PersistenceFuture<'a, Vec<i64>>;

    fn departments<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<DiagnosticDepartmentRecord>>;
}
