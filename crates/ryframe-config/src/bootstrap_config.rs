use std::fmt;

use serde::Deserialize;

/// 显式初始化所需的配置；复杂度和必填约束由初始化调用方校验。
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BootstrapConfig {
    pub admin_password: Option<String>,
}

impl fmt::Debug for BootstrapConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapConfig")
            .field(
                "admin_password",
                &self.admin_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}
