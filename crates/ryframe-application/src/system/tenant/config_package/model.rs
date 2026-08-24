use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use super::super::super::CapabilityRequirement;

/// 清单中的资源计数；关联关系也计入容量上限。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfigResourceCounts {
    pub departments: usize,
    pub posts: usize,
    pub dict_types: usize,
    pub dict_data: usize,
    pub configs: usize,
    pub permissions: usize,
    pub menus: usize,
    pub roles: usize,
    pub role_permissions: usize,
    pub role_custom_departments: usize,
}

impl TenantConfigResourceCounts {
    pub(super) fn total(&self) -> AppResult<usize> {
        [
            self.departments,
            self.posts,
            self.dict_types,
            self.dict_data,
            self.configs,
            self.permissions,
            self.menus,
            self.roles,
            self.role_permissions,
            self.role_custom_departments,
        ]
        .into_iter()
        .try_fold(0usize, |total, count| {
            total
                .checked_add(count)
                .ok_or_else(|| AppError::Validation("配置包项目数量溢出".into()))
        })
    }
}

/// 目录摘要用于快速识别目标环境缺失的权限或页面注册项。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfigCatalogSummary {
    pub count: usize,
    pub sha256: String,
}

/// 配置包清单。资源完整性基于 ZIP 中 `resources.json` 的原始字节计算。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfigPackageManifest {
    pub schema: String,
    pub source_app_version: String,
    pub source_tenant_key: String,
    pub source_tenant_name: String,
    pub generated_at: DateTime<Utc>,
    pub resource_counts: TenantConfigResourceCounts,
    pub item_count: usize,
    pub resources_sha256: String,
    /// 包内权限/菜单真实依赖的产品能力版本；仅用于目标兼容校验。
    pub required_capabilities: Vec<CapabilityRequirement>,
    pub required_permissions: TenantConfigCatalogSummary,
    pub required_page_routes: TenantConfigCatalogSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDepartment {
    /// 从根部门开始的完整名称路径，不能使用数据库 ID。
    pub path: Vec<String>,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePost {
    pub code: String,
    pub name: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDictType {
    pub code: String,
    pub name: String,
    pub status: String,
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDictData {
    pub type_code: String,
    pub value: String,
    pub label: String,
    pub sort: i32,
    pub status: String,
    pub css_class: Option<String>,
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableConfig {
    pub key: String,
    pub name: String,
    pub value: String,
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePermission {
    pub code: String,
    pub name: String,
    pub parent_code: Option<String>,
    pub permission_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableMenu {
    /// 页面或目录使用 route_key；操作使用父菜单稳定键与权限代码生成的无歧义键。
    pub stable_key: String,
    pub parent_stable_key: Option<String>,
    pub name: String,
    pub menu_type: String,
    pub permission_code: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRole {
    pub code: String,
    pub name: String,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub permission_codes: Vec<String>,
    pub custom_department_paths: Vec<Vec<String>>,
}

/// 配置包的全部资源。模型中不存在任何数据库 ID 字段。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfigPackageResources {
    pub departments: Vec<PortableDepartment>,
    pub posts: Vec<PortablePost>,
    pub dict_types: Vec<PortableDictType>,
    pub dict_data: Vec<PortableDictData>,
    pub configs: Vec<PortableConfig>,
    pub permissions: Vec<PortablePermission>,
    pub menus: Vec<PortableMenu>,
    pub roles: Vec<PortableRole>,
}
