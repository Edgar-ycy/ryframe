//! 文件上传、下载和清理所需的持久化端口。

mod cleanup;
mod download;
mod upload;

pub use cleanup::{
    FILE_DEL_FLAG_NORMAL, FILE_UPLOAD_STATUS_CLEANUP, FILE_UPLOAD_STATUS_PENDING,
    FILE_UPLOAD_STATUS_READY, FileCleanupPersistencePort, FileCleanupRecord,
    FileCleanupTransaction,
};
pub use download::{FileDownloadPersistencePort, FileDownloadRecord};
pub use upload::{
    FileUploadCommitMode, FileUploadPersistencePort, FileUploadRecord, FileUploadTransaction,
};
