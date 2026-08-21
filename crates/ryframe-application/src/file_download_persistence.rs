use crate::PersistenceFuture;

#[derive(Debug, Eq, PartialEq)]
pub struct FileDownloadRecord {
    pub bucket: String,
    pub storage_path: String,
    pub original_name: String,
    pub content_type: String,
}

/// 文件下载所需的持久化读取端口。
pub trait FileDownloadPersistencePort: Send + Sync {
    fn find_by_storage_path<'a>(
        &'a self,
        tenant_id: &'a str,
        bucket: &'a str,
        storage_path: &'a str,
    ) -> PersistenceFuture<'a, Option<FileDownloadRecord>>;

    fn find_ready_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        expected_bucket: &'a str,
    ) -> PersistenceFuture<'a, Option<FileDownloadRecord>>;
}
