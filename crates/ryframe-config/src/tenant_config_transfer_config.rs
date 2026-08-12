use serde::Deserialize;

/// 租户配置包生成、上传、应用和回滚的容量与租约限制。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfigTransferConfig {
    #[serde(default = "default_max_package_bytes")]
    pub max_package_bytes: usize,
    #[serde(default = "default_max_uncompressed_bytes")]
    pub max_uncompressed_bytes: usize,
    #[serde(default = "default_max_items")]
    pub max_items: usize,
    #[serde(default = "default_artifact_hours")]
    pub artifact_hours: u32,
    #[serde(default = "default_rollback_hours")]
    pub rollback_hours: u32,
    #[serde(default = "default_lease_seconds")]
    pub lease_seconds: u64,
    #[serde(default = "default_max_runtime_seconds")]
    pub max_runtime_seconds: u32,
}

impl Default for TenantConfigTransferConfig {
    fn default() -> Self {
        Self {
            max_package_bytes: default_max_package_bytes(),
            max_uncompressed_bytes: default_max_uncompressed_bytes(),
            max_items: default_max_items(),
            artifact_hours: default_artifact_hours(),
            rollback_hours: default_rollback_hours(),
            lease_seconds: default_lease_seconds(),
            max_runtime_seconds: default_max_runtime_seconds(),
        }
    }
}

impl TenantConfigTransferConfig {
    /// 校验配置包容量、保留窗口、租约和后台任务运行时长。
    pub fn validate(&self, upload_file_max_bytes: usize) -> Result<(), String> {
        if self.max_package_bytes == 0 || self.max_package_bytes > upload_file_max_bytes {
            return Err(
                "tenant_config_transfer.max_package_bytes 必须大于 0 且不超过 upload.file_max_bytes"
                    .into(),
            );
        }
        if self.max_uncompressed_bytes < self.max_package_bytes
            || self.max_uncompressed_bytes > 100 * 1024 * 1024
        {
            return Err(
                "tenant_config_transfer.max_uncompressed_bytes 必须不小于包大小且不超过 100 MiB"
                    .into(),
            );
        }
        if !(1..=10_000).contains(&self.max_items) {
            return Err("tenant_config_transfer.max_items 必须在 1 到 10000 之间".into());
        }
        for (name, hours) in [
            ("artifact_hours", self.artifact_hours),
            ("rollback_hours", self.rollback_hours),
        ] {
            if !(1..=8_760).contains(&hours) {
                return Err(format!(
                    "tenant_config_transfer.{name} 必须在 1 到 8760 小时之间"
                ));
            }
        }
        if !(30..=3_600).contains(&self.lease_seconds) {
            return Err("tenant_config_transfer.lease_seconds 必须在 30 到 3600 秒之间".into());
        }
        if !(60..=86_400).contains(&self.max_runtime_seconds) {
            return Err(
                "tenant_config_transfer.max_runtime_seconds 必须在 60 到 86400 秒之间".into(),
            );
        }
        Ok(())
    }
}

const fn default_max_package_bytes() -> usize {
    5 * 1024 * 1024
}

const fn default_max_uncompressed_bytes() -> usize {
    20 * 1024 * 1024
}

const fn default_max_items() -> usize {
    10_000
}

const fn default_artifact_hours() -> u32 {
    168
}

const fn default_rollback_hours() -> u32 {
    168
}

const fn default_lease_seconds() -> u64 {
    300
}

const fn default_max_runtime_seconds() -> u32 {
    1_800
}
