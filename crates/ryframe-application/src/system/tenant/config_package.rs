use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{CAPABILITY_CATALOG, CapabilityRequirement};

mod format;

/// 租户配置包的固定协议标识。
pub const TENANT_CONFIG_PACKAGE_SCHEMA: &str = "ryframe.tenant-config/v2";

/// 配置包仅允许包含的清单文件名。
const MANIFEST_FILE_NAME: &str = "manifest.json";

/// 配置包仅允许包含的资源文件名。
const RESOURCES_FILE_NAME: &str = "resources.json";

/// 防止高压缩比输入消耗不成比例的 CPU 与内存。
const MAX_COMPRESSION_RATIO: u64 = 100;

/// JSON 最大嵌套深度。配置资源无需任意深度结构。
const MAX_JSON_DEPTH: usize = 32;

const TENANT_KEY_MAX_CHARS: usize = 64;
const TENANT_NAME_MAX_CHARS: usize = 128;
const APP_VERSION_MAX_CHARS: usize = 32;
const NAME_MAX_CHARS: usize = 64;
const CONFIG_NAME_MAX_CHARS: usize = 128;
const STABLE_CODE_MAX_BYTES: usize = 64;
const CONFIG_KEY_MAX_BYTES: usize = 128;
const PERMISSION_CODE_MAX_BYTES: usize = 128;
const ROUTE_KEY_MAX_BYTES: usize = 100;
const MENU_STABLE_KEY_MAX_BYTES: usize = 384;
const TRANSFER_STABLE_KEY_MAX_CHARS: usize = 384;
const CONFIG_VALUE_MAX_CHARS: usize = 512;
const REMARK_MAX_CHARS: usize = 512;
const ICON_MAX_CHARS: usize = 128;

/// 受控配置包解析和生成的容量边界。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantConfigPackageLimits {
    pub max_package_bytes: usize,
    pub max_uncompressed_bytes: usize,
    pub max_items: usize,
}

impl TenantConfigPackageLimits {
    pub fn new(
        max_package_bytes: usize,
        max_uncompressed_bytes: usize,
        max_items: usize,
    ) -> AppResult<Self> {
        if max_package_bytes == 0 {
            return Err(AppError::Validation("配置包大小限制必须大于零".into()));
        }
        if max_uncompressed_bytes < max_package_bytes {
            return Err(AppError::Validation(
                "配置包解压大小限制不能小于压缩包大小限制".into(),
            ));
        }
        if max_items == 0 {
            return Err(AppError::Validation("配置包项目数量限制必须大于零".into()));
        }
        Ok(Self {
            max_package_bytes,
            max_uncompressed_bytes,
            max_items,
        })
    }
}

impl From<&crate::TenantConfigTransferPolicy> for TenantConfigPackageLimits {
    fn from(config: &crate::TenantConfigTransferPolicy) -> Self {
        Self {
            max_package_bytes: config.max_package_bytes,
            max_uncompressed_bytes: config.max_uncompressed_bytes,
            max_items: config.max_items,
        }
    }
}

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
    fn total(&self) -> AppResult<usize> {
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

impl TenantConfigPackageResources {
    pub fn counts(&self) -> TenantConfigResourceCounts {
        TenantConfigResourceCounts {
            departments: self.departments.len(),
            posts: self.posts.len(),
            dict_types: self.dict_types.len(),
            dict_data: self.dict_data.len(),
            configs: self.configs.len(),
            permissions: self.permissions.len(),
            menus: self.menus.len(),
            roles: self.roles.len(),
            role_permissions: self
                .roles
                .iter()
                .map(|role| role.permission_codes.len())
                .sum(),
            role_custom_departments: self
                .roles
                .iter()
                .map(|role| role.custom_department_paths.len())
                .sum(),
        }
    }

    /// 按稳定业务键排序，得到可重复哈希和差异比较的规范表示。
    pub fn canonicalize(&mut self) {
        self.departments
            .sort_by(|left, right| left.path.cmp(&right.path));
        self.posts.sort_by(|left, right| left.code.cmp(&right.code));
        self.dict_types
            .sort_by(|left, right| left.code.cmp(&right.code));
        self.dict_data.sort_by(|left, right| {
            (&left.type_code, &left.value).cmp(&(&right.type_code, &right.value))
        });
        self.configs.sort_by(|left, right| left.key.cmp(&right.key));
        self.permissions
            .sort_by(|left, right| left.code.cmp(&right.code));
        self.menus
            .sort_by(|left, right| left.stable_key.cmp(&right.stable_key));
        for role in &mut self.roles {
            role.permission_codes.sort();
            role.custom_department_paths.sort();
        }
        self.roles.sort_by(|left, right| left.code.cmp(&right.code));
    }

    fn validate(&self, limits: TenantConfigPackageLimits) -> AppResult<()> {
        let counts = self.counts();
        if counts.total()? > limits.max_items {
            return Err(AppError::Validation(format!(
                "配置包项目数量超过限制（最大 {} 项）",
                limits.max_items
            )));
        }

        unique_by(
            self.departments
                .iter()
                .map(|item| normalized_path_key(&item.path)),
            "部门完整路径重复",
        )?;
        let department_paths = self
            .departments
            .iter()
            .map(|item| normalized_path_key(&item.path))
            .collect::<BTreeSet<_>>();
        for department in &self.departments {
            validate_path(&department.path, "部门路径")?;
            validate_department_stable_key(&department.path)?;
            validate_status(&department.status, "部门状态")?;
            validate_optional_text(&department.remark, REMARK_MAX_CHARS, "部门备注")?;
            let normalized_path = normalized_path_key(&department.path);
            if normalized_path.len() > 1
                && !department_paths.contains(&normalized_path[..normalized_path.len() - 1])
            {
                return Err(AppError::Validation(format!(
                    "部门路径 {} 缺少父部门",
                    department.path.join("/")
                )));
            }
        }
        unique_by(
            self.posts.iter().map(|item| collation_key(&item.code)),
            "岗位代码重复",
        )?;
        unique_by(
            self.dict_types.iter().map(|item| collation_key(&item.code)),
            "字典类型代码重复",
        )?;
        unique_by(
            self.dict_data
                .iter()
                .map(|item| (collation_key(&item.type_code), collation_key(&item.value))),
            "字典数据稳定键重复",
        )?;
        unique_by(
            self.configs.iter().map(|item| collation_key(&item.key)),
            "参数键重复",
        )?;
        unique_by(
            self.permissions
                .iter()
                .map(|item| collation_key(&item.code)),
            "权限代码重复",
        )?;
        unique_by(
            self.menus
                .iter()
                .map(|item| collation_key(&item.stable_key)),
            "菜单稳定键重复",
        )?;
        unique_by(
            self.roles.iter().map(|item| collation_key(&item.code)),
            "角色代码重复",
        )?;

        let dict_types = self
            .dict_types
            .iter()
            .map(|item| collation_key(&item.code))
            .collect::<BTreeSet<_>>();
        for item in &self.dict_data {
            validate_stable_code(&item.type_code, STABLE_CODE_MAX_BYTES, "字典类型代码")?;
            validate_stable_code(&item.value, STABLE_CODE_MAX_BYTES, "字典数据值")?;
            validate_text(&item.label, NAME_MAX_CHARS, "字典数据标签")?;
            validate_optional_text(&item.css_class, NAME_MAX_CHARS, "字典数据样式")?;
            validate_optional_text(&item.remark, REMARK_MAX_CHARS, "字典数据备注")?;
            if !dict_types.contains(&collation_key(&item.type_code)) {
                return Err(AppError::Validation(format!(
                    "字典数据引用了不存在的字典类型：{}",
                    item.type_code
                )));
            }
        }

        let permissions = self
            .permissions
            .iter()
            .map(|item| collation_key(&item.code))
            .collect::<BTreeSet<_>>();
        let mut permission_parent_by_code = std::collections::BTreeMap::new();
        for item in &self.permissions {
            validate_stable_code(&item.code, PERMISSION_CODE_MAX_BYTES, "权限代码")?;
            validate_text(&item.name, NAME_MAX_CHARS, "权限名称")?;
            validate_optional_text(&item.icon, NAME_MAX_CHARS, "权限图标")?;
            validate_status(&item.status, "权限状态")?;
            if !matches!(item.permission_type.as_str(), "api" | "menu") {
                return Err(AppError::Validation(format!(
                    "权限 {} 的类型不受支持",
                    item.code
                )));
            }
            if permission_contains_wildcard(&item.code) {
                return Err(AppError::Validation("配置包不能包含超级通配权限".into()));
            }
            if let Some(parent) = item.parent_code.as_deref()
                && !permissions.contains(&collation_key(parent))
            {
                return Err(AppError::Validation(format!(
                    "权限 {} 引用了不存在的父权限 {}",
                    item.code, parent
                )));
            }
            permission_parent_by_code.insert(
                collation_key(&item.code),
                item.parent_code.as_deref().map(collation_key),
            );
        }
        validate_parent_graph(&permission_parent_by_code, "权限目录")?;

        let menus = self
            .menus
            .iter()
            .map(|item| collation_key(&item.stable_key))
            .collect::<BTreeSet<_>>();
        let menu_types = self
            .menus
            .iter()
            .map(|item| (collation_key(&item.stable_key), item.menu_type.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut menu_parent_by_key = std::collections::BTreeMap::new();
        for item in &self.menus {
            validate_stable_text(&item.stable_key, MENU_STABLE_KEY_MAX_BYTES, "菜单稳定键")?;
            validate_text(&item.name, NAME_MAX_CHARS, "菜单名称")?;
            validate_optional_text(&item.icon, ICON_MAX_CHARS, "菜单图标")?;
            validate_optional_text(&item.remark, REMARK_MAX_CHARS, "菜单备注")?;
            validate_status(&item.status, "菜单状态")?;
            if let Some(parent) = item.parent_stable_key.as_deref()
                && !menus.contains(&collation_key(parent))
            {
                return Err(AppError::Validation(format!(
                    "菜单 {} 引用了不存在的父菜单 {}",
                    item.stable_key, parent
                )));
            }
            if let Some(parent) = item.parent_stable_key.as_deref()
                && menu_types.get(&collation_key(parent)).copied() == Some("F")
            {
                return Err(AppError::Validation(format!(
                    "菜单 {} 不能将操作菜单作为父菜单",
                    item.stable_key
                )));
            }
            match item.menu_type.as_str() {
                "M" | "C" => {
                    let route_key = item.route_key.as_deref().ok_or_else(|| {
                        AppError::Validation(format!(
                            "目录或页面菜单 {} 缺少 route_key",
                            item.stable_key
                        ))
                    })?;
                    validate_stable_code(route_key, ROUTE_KEY_MAX_BYTES, "页面 route_key")?;
                    if item.stable_key != route_menu_stable_key(route_key) {
                        return Err(AppError::Validation(format!(
                            "菜单 {} 的稳定键与 route_key 不匹配",
                            item.stable_key
                        )));
                    }
                    if item.menu_type == "C" && item.permission_code.is_none() {
                        return Err(AppError::Validation(format!(
                            "页面菜单 {} 必须绑定权限代码",
                            item.stable_key
                        )));
                    }
                    if let Some(permission) = item.permission_code.as_deref()
                        && !permissions.contains(&collation_key(permission))
                    {
                        return Err(AppError::Validation(format!(
                            "目录或页面菜单引用了不存在的权限：{}",
                            permission
                        )));
                    }
                }
                "F" => {
                    if item.route_key.is_some() {
                        return Err(AppError::Validation(format!(
                            "操作菜单 {} 不能声明 route_key",
                            item.stable_key
                        )));
                    }
                    let parent = item.parent_stable_key.as_deref().ok_or_else(|| {
                        AppError::Validation("操作菜单必须声明父菜单稳定键".into())
                    })?;
                    let permission = item
                        .permission_code
                        .as_deref()
                        .ok_or_else(|| AppError::Validation("操作菜单必须绑定权限代码".into()))?;
                    if !permissions.contains(&collation_key(permission)) {
                        return Err(AppError::Validation(format!(
                            "操作菜单引用了不存在的权限：{}",
                            permission
                        )));
                    }
                    if item.stable_key != action_menu_stable_key(parent, permission) {
                        return Err(AppError::Validation(format!(
                            "操作菜单 {} 的稳定键不合法",
                            item.stable_key
                        )));
                    }
                }
                _ => {
                    return Err(AppError::Validation(format!(
                        "菜单 {} 的类型不受支持",
                        item.stable_key
                    )));
                }
            }
            menu_parent_by_key.insert(
                collation_key(&item.stable_key),
                item.parent_stable_key.as_deref().map(collation_key),
            );
        }
        validate_parent_graph(&menu_parent_by_key, "菜单目录")?;

        for config in &self.configs {
            validate_stable_code(&config.key, CONFIG_KEY_MAX_BYTES, "参数键")?;
            validate_text(&config.name, CONFIG_NAME_MAX_CHARS, "参数名称")?;
            validate_text(&config.value, CONFIG_VALUE_MAX_CHARS, "参数值")?;
            validate_optional_text(&config.remark, REMARK_MAX_CHARS, "参数备注")?;
            if is_sensitive_config_key(&config.key) {
                return Err(AppError::Validation(format!(
                    "敏感参数不能进入配置包：{}",
                    config.key
                )));
            }
        }
        for post in &self.posts {
            validate_stable_code(&post.code, STABLE_CODE_MAX_BYTES, "岗位代码")?;
            validate_text(&post.name, NAME_MAX_CHARS, "岗位名称")?;
            validate_optional_text(&post.remark, REMARK_MAX_CHARS, "岗位备注")?;
            validate_status(&post.status, "岗位状态")?;
        }
        for dict_type in &self.dict_types {
            validate_stable_code(&dict_type.code, STABLE_CODE_MAX_BYTES, "字典类型代码")?;
            validate_text(&dict_type.name, NAME_MAX_CHARS, "字典类型名称")?;
            validate_optional_text(&dict_type.remark, REMARK_MAX_CHARS, "字典类型备注")?;
            validate_status(&dict_type.status, "字典类型状态")?;
        }
        for dict_data in &self.dict_data {
            validate_status(&dict_data.status, "字典数据状态")?;
        }
        for role in &self.roles {
            validate_stable_code(&role.code, STABLE_CODE_MAX_BYTES, "角色代码")?;
            validate_text(&role.name, NAME_MAX_CHARS, "角色名称")?;
            validate_optional_text(&role.remark, REMARK_MAX_CHARS, "角色备注")?;
            validate_status(&role.status, "角色状态")?;
            if !matches!(role.data_scope.as_str(), "1" | "2" | "3" | "4" | "5") {
                return Err(AppError::Validation(format!(
                    "角色 {} 的数据范围不受支持",
                    role.code
                )));
            }
            if role.data_scope != "2" && !role.custom_department_paths.is_empty() {
                return Err(AppError::Validation(format!(
                    "非自定义数据范围角色 {} 不能包含自定义部门",
                    role.code
                )));
            }
            unique_by(
                role.permission_codes.iter().map(|code| collation_key(code)),
                "角色权限代码重复",
            )?;
            unique_by(
                role.custom_department_paths
                    .iter()
                    .map(|path| normalized_path_key(path)),
                "角色自定义部门路径重复",
            )?;
            for code in &role.permission_codes {
                validate_stable_code(code, PERMISSION_CODE_MAX_BYTES, "角色权限代码")?;
                if !permissions.contains(&collation_key(code)) {
                    return Err(AppError::Validation(format!(
                        "角色 {} 引用了不存在的权限 {}",
                        role.code, code
                    )));
                }
            }
            for path in &role.custom_department_paths {
                validate_path(path, "角色自定义部门路径")?;
                validate_department_stable_key(path)?;
                if !department_paths.contains(&normalized_path_key(path)) {
                    return Err(AppError::Validation(format!(
                        "角色 {} 引用了不存在的部门路径",
                        role.code
                    )));
                }
            }
        }
        Ok(())
    }
}

fn permission_contains_wildcard(code: &str) -> bool {
    code.split(':').any(|segment| segment == "*")
}

/// 成功生成的配置包及其规范资源表示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedTenantConfigPackage {
    pub manifest: TenantConfigPackageManifest,
    pub resources: TenantConfigPackageResources,
    pub canonical_resources: Vec<u8>,
    pub data: Vec<u8>,
    pub package_sha256: String,
}

/// 成功解析并完成安全校验的配置包。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedTenantConfigPackage {
    pub manifest: TenantConfigPackageManifest,
    pub resources: TenantConfigPackageResources,
    pub canonical_resources: Vec<u8>,
    pub package_sha256: String,
}

/// 生成租户配置包时写入清单的来源信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantConfigPackageSource {
    pub tenant_key: String,
    pub tenant_name: String,
    pub app_version: String,
    pub generated_at: DateTime<Utc>,
}

/// 在阻塞线程中构造只含两个受控文件的配置包。
pub async fn build_tenant_config_package(
    archive: Arc<dyn crate::ports::tenant_config::TenantConfigArchivePort>,
    resources: TenantConfigPackageResources,
    required_capabilities: Vec<CapabilityRequirement>,
    source: TenantConfigPackageSource,
    limits: TenantConfigPackageLimits,
) -> AppResult<GeneratedTenantConfigPackage> {
    tokio::task::spawn_blocking(move || {
        format::build_package_blocking(
            archive.as_ref(),
            resources,
            required_capabilities,
            source,
            limits,
        )
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "租户配置包生成阻塞任务失败");
        AppError::Internal("租户配置包生成任务失败".into())
    })?
}

/// 在阻塞线程中解析并校验受控配置包。
pub async fn parse_tenant_config_package(
    archive: Arc<dyn crate::ports::tenant_config::TenantConfigArchivePort>,
    data: Vec<u8>,
    limits: TenantConfigPackageLimits,
) -> AppResult<ParsedTenantConfigPackage> {
    let (parsed, _) = parse_tenant_config_package_with_source(archive, data, limits).await?;
    Ok(parsed)
}

pub(super) async fn parse_tenant_config_package_with_source(
    archive: Arc<dyn crate::ports::tenant_config::TenantConfigArchivePort>,
    data: Vec<u8>,
    limits: TenantConfigPackageLimits,
) -> AppResult<(ParsedTenantConfigPackage, Vec<u8>)> {
    tokio::task::spawn_blocking(move || {
        let parsed = format::parse_package_blocking(archive.as_ref(), &data, limits)?;
        Ok((parsed, data))
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "租户配置包解析阻塞任务失败");
        AppError::Internal("租户配置包解析任务失败".into())
    })?
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn catalog_summary<'a>(values: impl Iterator<Item = &'a str>) -> TenantConfigCatalogSummary {
    let values = values.collect::<BTreeSet<_>>();
    let canonical = values.iter().copied().collect::<Vec<_>>().join("\n");
    TenantConfigCatalogSummary {
        count: values.len(),
        sha256: sha256_hex(canonical.as_bytes()),
    }
}

fn required_permission_summary(
    resources: &TenantConfigPackageResources,
) -> TenantConfigCatalogSummary {
    catalog_summary(resources.permissions.iter().map(|item| item.code.as_str()))
}

fn required_route_summary(resources: &TenantConfigPackageResources) -> TenantConfigCatalogSummary {
    catalog_summary(
        resources
            .menus
            .iter()
            .filter_map(|item| item.route_key.as_deref()),
    )
}

fn validate_required_capabilities(
    requirements: &[CapabilityRequirement],
    resources: &TenantConfigPackageResources,
) -> AppResult<()> {
    let mut canonical = requirements.to_vec();
    canonical.sort();
    if canonical != requirements {
        return Err(AppError::Validation(
            "配置包 required_capabilities 必须按 code/variant/schema_version 排序".into(),
        ));
    }
    let mut declared_codes = BTreeSet::new();
    for requirement in requirements {
        validate_stable_code(&requirement.code, STABLE_CODE_MAX_BYTES, "能力代码")?;
        validate_stable_code(&requirement.variant, STABLE_CODE_MAX_BYTES, "能力 variant")?;
        if requirement.schema_version <= 0 {
            return Err(AppError::Validation(
                "能力 schema_version 必须是正整数".into(),
            ));
        }
        if !declared_codes.insert(requirement.code.as_str()) {
            return Err(AppError::Validation(format!(
                "配置包重复声明能力 {}",
                requirement.code
            )));
        }
        let descriptor = CAPABILITY_CATALOG
            .iter()
            .find(|descriptor| descriptor.code == requirement.code)
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "配置包声明了当前版本未知的能力 {}",
                    requirement.code
                ))
            })?;
        if !descriptor.variants.iter().any(|variant| {
            variant.code == requirement.variant
                && variant.schema_version == requirement.schema_version
        }) {
            return Err(AppError::Validation(format!(
                "配置包能力 {} 的 variant/schema 不受当前版本支持",
                requirement.code
            )));
        }
    }

    let mut involved_codes = BTreeSet::new();
    for descriptor in CAPABILITY_CATALOG {
        let uses_permission = resources.permissions.iter().any(|permission| {
            descriptor
                .permission_codes
                .contains(&permission.code.as_str())
        }) || resources.roles.iter().any(|role| {
            role.permission_codes
                .iter()
                .any(|permission| descriptor.permission_codes.contains(&permission.as_str()))
        }) || resources.menus.iter().any(|menu| {
            menu.permission_code
                .as_deref()
                .is_some_and(|permission| descriptor.permission_codes.contains(&permission))
        });
        let uses_route = resources.menus.iter().any(|menu| {
            menu.route_key
                .as_deref()
                .is_some_and(|route| descriptor.route_keys.contains(&route))
        });
        if uses_permission || uses_route {
            involved_codes.insert(descriptor.code);
        }
    }
    if involved_codes != declared_codes {
        return Err(AppError::Validation(
            "配置包 required_capabilities 与实际权限/菜单资源不一致".into(),
        ));
    }
    Ok(())
}

fn route_menu_stable_key(route_key: &str) -> String {
    format!("route:{}:{route_key}", route_key.len())
}

fn action_menu_stable_key(parent_stable_key: &str, permission_code: &str) -> String {
    format!(
        "action:{}:{parent_stable_key}:{}:{permission_code}",
        parent_stable_key.len(),
        permission_code.len()
    )
}

/// 保守识别不得跨环境迁移的敏感参数键。
pub fn is_sensitive_config_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    const SENSITIVE_MARKERS: [&str; 10] = [
        "password",
        "passwd",
        "passphrase",
        "secret",
        "token",
        "credential",
        "privatekey",
        "apikey",
        "accesskey",
        "signingkey",
    ];
    SENSITIVE_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn validate_path(path: &[String], label: &str) -> AppResult<()> {
    if path.is_empty() {
        return Err(AppError::Validation(format!("{label}不能为空")));
    }
    for segment in path {
        validate_text(segment, NAME_MAX_CHARS, label)?;
        if segment.trim() != segment {
            return Err(AppError::Validation(format!("{label}格式无效")));
        }
    }
    Ok(())
}

fn validate_department_stable_key(path: &[String]) -> AppResult<()> {
    let stable_key = path
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("/");
    if stable_key.chars().count() > TRANSFER_STABLE_KEY_MAX_CHARS {
        return Err(AppError::Validation(format!(
            "部门完整路径生成的稳定键不能超过 {TRANSFER_STABLE_KEY_MAX_CHARS} 个字符"
        )));
    }
    Ok(())
}

fn validate_stable_text(value: &str, max_bytes: usize, label: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > max_bytes
        || value.contains('\0')
    {
        return Err(AppError::Validation(format!("{label}格式无效")));
    }
    Ok(())
}

fn validate_stable_code(value: &str, max_bytes: usize, label: &str) -> AppResult<()> {
    validate_stable_text(value, max_bytes, label)?;
    if value.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
        return Err(AppError::Validation(format!(
            "{label}只能使用 ASCII 可见字符"
        )));
    }
    Ok(())
}

fn validate_text(value: &str, max_chars: usize, label: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().count() > max_chars {
        return Err(AppError::Validation(format!(
            "{label}不能为空且不能超过 {max_chars} 个字符"
        )));
    }
    Ok(())
}

fn validate_optional_text(value: &Option<String>, max_chars: usize, label: &str) -> AppResult<()> {
    if value
        .as_deref()
        .is_some_and(|value| value.contains('\0') || value.chars().count() > max_chars)
    {
        return Err(AppError::Validation(format!(
            "{label}不能超过 {max_chars} 个字符"
        )));
    }
    Ok(())
}

fn collation_key(value: &str) -> String {
    // 便携业务代码均受 ASCII 校验约束，小写化后与目标端的不区分大小写键语义一致。
    value.to_ascii_lowercase()
}

fn normalized_path_key(path: &[String]) -> Vec<String> {
    // 部门路径采用明确的逐段二进制语义，不使用 Rust 近似数据库排序规则。
    path.to_vec()
}

fn validate_status(value: &str, label: &str) -> AppResult<()> {
    if matches!(value, "0" | "1") {
        Ok(())
    } else {
        Err(AppError::Validation(format!("{label}无效")))
    }
}

fn unique_by<T>(values: impl IntoIterator<Item = T>, message: &str) -> AppResult<()>
where
    T: Ord,
{
    let mut unique = BTreeSet::new();
    for value in values {
        if !unique.insert(value) {
            return Err(AppError::Validation(message.to_owned()));
        }
    }
    Ok(())
}

fn validate_parent_graph(
    parents: &std::collections::BTreeMap<String, Option<String>>,
    label: &str,
) -> AppResult<()> {
    for node in parents.keys() {
        let mut current = Some(node.as_str());
        let mut visiting = BTreeSet::new();
        while let Some(value) = current {
            if !visiting.insert(value) {
                return Err(AppError::Validation(format!("{label}存在循环引用")));
            }
            current = parents.get(value).and_then(Option::as_deref);
        }
    }
    Ok(())
}
