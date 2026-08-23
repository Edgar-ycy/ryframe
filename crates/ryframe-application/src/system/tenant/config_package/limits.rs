use ryframe_kernel::{AppError, AppResult};

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
