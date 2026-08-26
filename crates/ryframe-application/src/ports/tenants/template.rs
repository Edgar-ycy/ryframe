#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantProvisioningTemplate {
    pub authorization: TenantAuthorizationTemplate,
    pub base_catalogs: TenantBaseCatalogTemplate,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TenantAuthorizationTemplate {
    pub permissions: Vec<TenantPermissionTemplate>,
    pub menus: Vec<TenantMenuTemplate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantPermissionTemplate {
    pub source_id: i64,
    pub name: String,
    pub code: String,
    pub parent_source_id: Option<i64>,
    pub permission_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub assign_admin: bool,
    pub assign_user: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantMenuTemplate {
    pub source_id: i64,
    pub name: String,
    pub parent_source_id: Option<i64>,
    pub menu_type: String,
    pub permission_source_id: Option<i64>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
    pub delete_flag: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TenantBaseCatalogTemplate {
    pub posts: Vec<TenantPostTemplate>,
    pub configs: Vec<TenantConfigTemplate>,
    pub dictionary_types: Vec<TenantDictionaryTypeTemplate>,
    pub dictionary_data: Vec<TenantDictionaryDataTemplate>,
    pub departments: Vec<TenantDepartmentTemplate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantPostTemplate {
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub delete_flag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantConfigTemplate {
    pub name: String,
    pub key: String,
    pub value: String,
    pub portable: bool,
    pub remark: Option<String>,
    pub delete_flag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantDictionaryTypeTemplate {
    pub name: String,
    pub code: String,
    pub status: String,
    pub remark: Option<String>,
    pub delete_flag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantDictionaryDataTemplate {
    pub type_code: String,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub status: String,
    pub css_class: Option<String>,
    pub remark: Option<String>,
    pub delete_flag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantDepartmentTemplate {
    pub source_id: i64,
    pub name: String,
    pub parent_source_id: Option<i64>,
    pub ancestor_source_ids: Vec<i64>,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub delete_flag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantProvisioningIdentity {
    pub tenant_id: String,
    pub admin_role_id: i64,
    pub user_role_id: i64,
    pub provisioned_at: chrono::DateTime<chrono::Utc>,
}
