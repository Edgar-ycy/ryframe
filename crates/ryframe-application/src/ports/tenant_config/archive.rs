use ryframe_kernel::AppResult;

/// 受控租户配置归档解出的两个固定文件。
#[derive(Debug, Eq, PartialEq)]
pub struct TenantConfigArchiveContents {
    pub manifest: Vec<u8>,
    pub resources: Vec<u8>,
}

/// 租户配置归档的非 SQL 出站端口。
pub trait TenantConfigArchivePort: Send + Sync {
    fn build(
        &self,
        manifest_name: &str,
        manifest: &[u8],
        resources_name: &str,
        resources: &[u8],
        max_package_bytes: usize,
    ) -> AppResult<Vec<u8>>;

    fn parse(
        &self,
        data: &[u8],
        manifest_name: &str,
        resources_name: &str,
        max_uncompressed_bytes: usize,
        max_compression_ratio: u64,
    ) -> AppResult<TenantConfigArchiveContents>;
}
