use serde::Deserialize;

/// 多租户运行模式配置。
///
/// 关闭多租户时仍保留数据库中的 `tenant_id` 隔离列，并将所有外部身份入口固定到
/// 内置 `system` 租户，避免改变既有表结构、唯一键和缓存命名空间。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiTenancyConfig {
    /// 是否允许客户端选择和管理多个租户。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// 单租户模式复用现有数据库引导数据使用的内置租户。
pub const SINGLE_TENANT_ID: &str = "system";

impl MultiTenancyConfig {
    /// 单租户模式下返回唯一允许使用的租户标识。
    pub fn fixed_tenant_id(&self) -> Option<&str> {
        (!self.enabled).then_some(SINGLE_TENANT_ID)
    }

    /// 判断已签发身份是否仍被当前运行模式允许。
    pub fn allows_tenant(&self, tenant_id: &str) -> bool {
        self.enabled || tenant_id == SINGLE_TENANT_ID
    }
}

impl Default for MultiTenancyConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
        }
    }
}

const fn default_enabled() -> bool {
    true
}
