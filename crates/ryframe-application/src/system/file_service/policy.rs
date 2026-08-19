/// 文件上传业务策略。
#[derive(Debug, Clone)]
pub struct UploadPolicy {
    pub max_file_size: u64,
    pub allowed_extensions: Vec<String>,
}

impl Default for UploadPolicy {
    fn default() -> Self {
        Self {
            max_file_size: 10 * 1024 * 1024,
            allowed_extensions: [
                "jpg", "jpeg", "png", "gif", "bmp", "webp", "pdf", "doc", "docx", "xls", "xlsx",
                "txt", "zip", "rar", "7z",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}
