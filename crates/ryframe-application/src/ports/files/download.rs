#[derive(Debug, Eq, PartialEq)]
pub struct FileDownloadRecord {
    pub bucket: String,
    pub storage_path: String,
    pub original_name: String,
    pub content_type: String,
}

/// 文件下载所需的持久化读取端口。
#[async_trait::async_trait]
pub trait FileDownloadPersistencePort: Send + Sync {
    async fn find_by_storage_path<'a>(
        &'a self,
        tenant_id: &'a str,
        bucket: &'a str,
        storage_path: &'a str,
    ) -> ryframe_kernel::AppResult<Option<FileDownloadRecord>>;

    async fn find_ready_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        expected_bucket: &'a str,
    ) -> ryframe_kernel::AppResult<Option<FileDownloadRecord>>;
}
