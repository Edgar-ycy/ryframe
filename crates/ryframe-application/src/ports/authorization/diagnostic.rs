use chrono::{DateTime, Utc};

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

#[async_trait::async_trait]
pub trait AuthorizationDiagnosticReadPort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn user_tenant_id(&self, user_id: i64) -> ryframe_kernel::AppResult<Option<String>>;

    async fn assigned_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticRoleRecord>>;

    async fn permissions<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticPermissionRecord>>;

    async fn role_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticPermissionRecord>>;

    async fn menus<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticMenuRecord>>;

    async fn accessible_menu_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> ryframe_kernel::AppResult<Vec<i64>>;

    async fn departments<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<DiagnosticDepartmentRecord>>;
}
