use std::sync::Arc;

use ryframe_kernel::{AppError, AppResult};

use super::super::CapabilityRequirement;

mod format;
mod limits;
mod model;
mod package;
mod validation;

pub use limits::TenantConfigPackageLimits;
pub use model::*;
pub use package::{
    GeneratedTenantConfigPackage, ParsedTenantConfigPackage, TenantConfigPackageSource,
    build_tenant_config_package, is_sensitive_config_key, parse_tenant_config_package,
};

use package::{
    required_permission_summary, required_route_summary, sha256_hex, validate_required_capabilities,
};

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
        validation::validate(self, limits)
    }
}
