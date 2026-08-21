use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

pub type FileContentFuture<'a> =
    Pin<Box<dyn Future<Output = AppResult<ProcessedFileContent>> + Send + 'a>>;

/// 已完成内容校验和可选压缩的上传文件。
pub struct ProcessedFileContent {
    pub original_name: String,
    pub data: Vec<u8>,
    pub file_name: String,
    pub content_type: String,
}

/// 文件内容校验与图片处理端口。
pub trait FileContentProcessor: Send + Sync {
    fn process(
        &self,
        original_name: String,
        data: Vec<u8>,
        compress: bool,
    ) -> FileContentFuture<'_>;
}
